use super::*;
use base64::Engine;

#[tokio::test]
async fn run_attachment_file_rpc_binds_the_turn_and_preserves_the_response_shape() {
    let temporary = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temporary.path().join("storage.sqlite")).unwrap());
    let agent = AgentService::new_authorized_for_test(Arc::clone(&storage));
    storage.save_conversation(serde_json::from_value(json!({
        "id":"read-chat", "projectId":null, "modelId":null, "title":"read", "createdAt":1, "updatedAt":2,
        "pinnedAt":null, "archivedAt":null, "unreadAt":null,
        "messages":[
            {"id":"read-user", "role":"user", "content":"", "createdAt":1, "status":"sent", "agentRunJson":null, "uiStateJson":null},
            {"id":"read-assistant", "role":"assistant", "content":"", "createdAt":2, "status":"pending", "agentRunJson":null, "uiStateJson":null}
        ]
    })).unwrap()).unwrap();
    let import_id = storage
        .begin_attachment_import(mycopilot_core::AttachmentImportInput {
            id: "read-file".into(),
            kind: mycopilot_core::AgentInputAttachmentKind::File,
            name: "notes.txt".into(),
            mime_type: Some("text/plain".into()),
            pasted_text: None,
            size_bytes: 3,
        })
        .unwrap();
    storage
        .append_attachment_import(&import_id, 0, "YWJj")
        .unwrap();
    let attachment = storage.finish_attachment_import(&import_id).unwrap();
    storage
        .save_input_attachments("read-chat", "read-user", None, &[attachment], 1)
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                "read-run",
                "read-chat",
                "read-assistant",
            ),
            2,
            2,
        )
        .unwrap();
    let request = |params: Value| {
        let (notifications, _receiver) = crate::transport::outbound_channel();
        handle_request(
            &storage,
            &agent,
            notifications,
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: "storage.resolveRunAttachmentFile".into(),
                params: Some(params),
            },
        )
    };
    let params = json!({
        "conversationId":"read-chat",
        "assistantMessageId":"read-assistant",
        "filePath":"@attachments/read-file/notes.txt"
    });
    let response = request(params.clone());
    assert_eq!(response["result"]["name"], "notes.txt");
    assert_eq!(response["result"]["messageId"], "read-user");
    assert_eq!(response["result"]["conversationId"], "read-chat");
    assert_eq!(
        std::fs::read(response["result"]["path"].as_str().unwrap()).unwrap(),
        b"abc"
    );
    for (key, value) in [
        ("assistantMessageId", "read-user"),
        ("conversationId", "foreign-chat"),
        ("filePath", "@attachments/read-file/forged.txt"),
        ("attachmentId", "forged-override"),
    ] {
        let mut invalid = params.clone();
        invalid[key] = json!(value);
        assert!(request(invalid)["error"].is_object());
    }
}

#[tokio::test]
async fn attachment_import_rpc_round_trip_validates_reference_and_preview_shapes() {
    let temporary = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temporary.path().join("storage.sqlite")).unwrap());
    let agent = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let request = |method: &str, params: Value| {
        let (notifications, _receiver) = crate::transport::outbound_channel();
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

#[tokio::test]
async fn pasted_text_restore_rpc_roundtrips_original_and_rejects_forged_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temporary.path().join("storage.sqlite")).unwrap());
    let agent = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let request = |method: &str, params: Value| {
        let (notifications, _receiver) = crate::transport::outbound_channel();
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
    let text = "  用户原文🙂\r\n尾部\t";
    let metadata = json!({"preview":"用户原文🙂", "characterCount":text.encode_utf16().count()});
    let begin = request(
        "storage.beginAttachmentImport",
        json!({
            "id":"wire-paste", "kind":"file", "name":"pasted-text.txt", "mimeType":"text/plain",
            "sizeBytes":text.len(), "pastedText":metadata,
        }),
    );
    let import_id = begin["result"]["importId"].as_str().unwrap();
    assert!(request("storage.appendAttachmentImport", json!({"importId":import_id,"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(text)}))["error"].is_null());
    let finish = request(
        "storage.finishAttachmentImport",
        json!({"importId":import_id}),
    );
    let attachment = finish["result"].clone();
    assert_eq!(attachment["pastedText"], metadata);
    assert_eq!(
        request(
            "storage.loadInputAttachmentText",
            json!({"attachment":attachment})
        )["result"],
        json!({"text":text})
    );
    let mut forged = attachment.clone();
    forged["id"] = json!("another-attachment");
    assert!(request(
        "storage.loadInputAttachmentText",
        json!({"attachment":forged})
    )["error"]
        .is_object());
    assert!(request(
        "storage.loadInputAttachmentText",
        json!({"attachment":attachment,"path":"/etc/passwd"})
    )["error"]
        .is_object());
    assert!(request(
        "storage.resolveAttachmentFile",
        json!({"attachmentId":"wire-paste","messageId":"missing"})
    )["error"]
        .is_object());
    assert!(request(
        "storage.resolveAttachmentFile",
        json!({"attachmentId":"wire-paste","messageId":"missing","path":"/etc/passwd"})
    )["error"]
        .is_object());
    storage.save_conversation(serde_json::from_value(json!({
        "id":"wire-chat", "projectId":null, "modelId":null, "title":"paste", "createdAt":1, "updatedAt":1,
        "pinnedAt":null, "archivedAt":null, "unreadAt":null,
        "messages":[{"id":"wire-message", "role":"user", "content":"", "createdAt":1, "status":"sent", "agentRunJson":null, "uiStateJson":null}]
    })).unwrap()).unwrap();
    let input: mycopilot_core::AgentInputAttachment = serde_json::from_value(attachment).unwrap();
    storage
        .save_input_attachments("wire-chat", "wire-message", None, &[input], 1)
        .unwrap();
    let resolved = request(
        "storage.resolveAttachmentFile",
        json!({"attachmentId":"wire-paste", "messageId":"wire-message"}),
    );
    assert_eq!(resolved["result"]["name"], "pasted-text.txt");
    assert_eq!(resolved["result"]["conversationId"], "wire-chat");
    assert_eq!(
        std::fs::read_to_string(resolved["result"]["path"].as_str().unwrap()).unwrap(),
        text
    );
}
