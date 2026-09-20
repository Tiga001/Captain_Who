use super::*;
use base64::Engine;

#[tokio::test]
async fn attachment_import_rpc_round_trip_validates_reference_and_preview_shapes() {
    let temporary = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temporary.path().join("storage.sqlite")).unwrap());
    let agent = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let request = |method: &str, params: Value| {
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        handle_request(
            &storage,
            &agent,
            notifications,
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: method.into(),
                params: Some(params),
            },
        )
    };
    let image = image::DynamicImage::new_rgb8(800, 400);
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let bytes = bytes.into_inner();
    let begin = request(
        "storage.beginAttachmentImport",
        json!({"id":"wire-image","kind":"image","name":"pixel.png","mimeType":"image/png","sizeBytes":bytes.len()}),
    );
    let import_id = begin["result"]["importId"]
        .as_str()
        .expect("opaque importId");
    let append = request(
        "storage.appendAttachmentImport",
        json!({"importId":import_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}),
    );
    assert_eq!(append["result"]["receivedBytes"], bytes.len());
    let finish = request(
        "storage.finishAttachmentImport",
        json!({"importId":import_id}),
    );
    let attachment = &finish["result"];
    assert_eq!(attachment["encoding"], "managed");
    assert_eq!(attachment["data"], import_id);
    let preview = request(
        "storage.loadInputAttachmentPreview",
        json!({"attachment":attachment}),
    );
    assert_eq!(preview["result"]["mimeType"], "image/png");
    let thumbnail = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(preview["result"]["data"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (256, 128));
    let display = request(
        "storage.loadInputAttachmentPreview",
        json!({"attachment":attachment,"purpose":"display"}),
    );
    let image = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(display["result"]["data"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!((image.width(), image.height()), (800, 400));
    assert!(request(
        "storage.cancelAttachmentImport",
        json!({"importId":import_id})
    )["result"]
        .is_null());
    // Cancellation cannot delete a reference that may already be persisted in a draft.
    assert_eq!(
        request(
            "storage.finishAttachmentImport",
            json!({"importId":import_id})
        )["result"],
        *attachment
    );
    let incomplete = request(
        "storage.beginAttachmentImport",
        json!({"id":"cancelled","kind":"file","name":"large.txt","mimeType":"text/plain","sizeBytes":100}),
    );
    let incomplete = incomplete["result"]["importId"].as_str().unwrap();
    assert!(request(
        "storage.cancelAttachmentImport",
        json!({"importId":incomplete})
    )["result"]
        .is_null());
    assert!(request(
        "storage.finishAttachmentImport",
        json!({"importId":incomplete})
    )["error"]
        .is_object());
    let mut forged = attachment.clone();
    forged["name"] = json!("different.png");
    assert!(request(
        "storage.loadInputAttachmentPreview",
        json!({"attachment":forged})
    )["error"]
        .is_object());
}
