use super::presentation_editor::{
    read_presentation_editor_plan, validate_presentation_editor_plan,
    validate_presentation_editor_script, PresentationEditorPlan,
    MAX_PRESENTATION_EDITOR_OPERATIONS, MAX_PRESENTATION_EDITOR_PLAN_BYTES,
};
use serde_json::{json, Value};
use std::fs;

fn valid_plan_value(target: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "source": {
            "type": "input",
            "mountPath": "source.pptx"
        },
        "destination": {
            "type": "output",
            "path": "outputs/source-edited.pptx"
        },
        "mode": "saveAs",
        "operations": [{
            "type": "set",
            "target": target,
            "properties": { "text": "Updated title" },
            "replacement": null,
            "force": false
        }]
    })
}

#[test]
fn editor_plan_uses_the_exact_sdk_camel_case_source_contract() {
    parse_and_validate(valid_plan_value("/slide[1]/shape[@id=1]"))
        .expect("the JSON emitted by the fixed SDK must deserialize");

    let mut legacy = valid_plan_value("/slide[1]/shape[@id=1]");
    legacy["source"] = json!({
        "type": "input",
        "mount_path": "source.pptx"
    });
    assert!(
        parse_and_validate(legacy).is_err(),
        "the Host must reject a snake_case source field the SDK never emits"
    );
}

fn parse_and_validate(value: Value) -> Result<PresentationEditorPlan, String> {
    let plan: PresentationEditorPlan =
        serde_json::from_value(value).map_err(|error| format!("deserialize: {error}"))?;
    validate_presentation_editor_plan(&plan)?;
    Ok(plan)
}

#[test]
fn editor_script_allows_only_the_materialized_template_edit_region_to_change() {
    const TEMPLATE: &str = include_str!("../skills/bundled/presentations/templates/editor.mjs");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("editor.mjs");

    fs::write(&path, TEMPLATE).unwrap();
    validate_presentation_editor_script(&path).expect("the exact bundled template must validate");

    let edited = TEMPLATE.replace(
        "    // Copy stable targets verbatim from the latest `office_presentation` inspect result.",
        "    deck.remove({ target: '/slide[1]/shape[@id=7]' })",
    );
    fs::write(&path, edited).unwrap();
    validate_presentation_editor_script(&path)
        .expect("the model-owned bounded edit region is intentionally editable");

    for tampered in [
        TEMPLATE.replace(
            "import { editPresentation, input, output } from '@mycopilot/presentation-sdk'",
            "import { readFile } from 'node:fs/promises'",
        ),
        format!("// injected wrapper byte\n{TEMPLATE}"),
        format!("{TEMPLATE}\nconsole.log(process.env)\n"),
        TEMPLATE.replace(
            "// BEGIN EDIT REGION",
            "// BEGIN EDIT REGION\n    // BEGIN EDIT REGION",
        ),
        TEMPLATE.replace("// END EDIT REGION", "// marker removed"),
    ] {
        fs::write(&path, tampered).unwrap();
        validate_presentation_editor_script(&path)
            .expect_err("any wrapper or marker change must require rematerialization");
    }
}

#[test]
fn editor_plan_accepts_only_exact_stable_element_anchors() {
    for target in [
        "/slide[1]/shape[@id=42]",
        "/slide[2]/shape[@id=7]",
        "/slide[3]/picture[@id=9]",
        "/slide[4]/table[@id=11]",
        "/slide[4]/table[@id=11]/row[2]/cell[3]",
        "/slide[5]/chart[@id=13]",
        "/slide[6]/connector[@id=15]",
    ] {
        parse_and_validate(valid_plan_value(target))
            .unwrap_or_else(|error| panic!("valid anchor `{target}` failed: {error}"));
    }

    for target in [
        "/slide[0]/shape[@id=1]",
        "/slide[1]/shape[1]",
        "/slide[1]/shape[@id=0]",
        "/slide[1]/textbox[@id=1]",
        "/slide[1]/evil[@id=1]",
        "/slide[1]/shape[@id=1]/child",
        "/slide[1]/shape[@id=1][name=Title]",
        "/slide[1]/shape[@id=1][@id=2]",
        "/slide[1]/shape[@id=1]]",
        "/slide[1]/shape[@id=1]\n/slide[2]",
        " /slide[1]/shape[@id=1]",
    ] {
        let error = parse_and_validate(valid_plan_value(target))
            .expect_err("unstable or injected anchor must fail closed");
        assert!(
            error.contains("stable")
                || error.contains("invalid")
                || error.contains("anchor")
                || error.contains("unsupported"),
            "unexpected error for `{target}`: {error}"
        );
    }
}

#[test]
fn editor_plan_accepts_one_terminal_whole_slide_operation_and_rejects_target_drift() {
    for operation in [
        json!({
            "type": "add",
            "parent": "/",
            "elementType": "slide",
            "copyFrom": null,
            "position": null,
            "properties": {
                "layout": "LAYOUT_WIDE",
                "title": "Appendix",
                "text": "Supporting detail",
                "background": "F8FAFC"
            },
            "force": false
        }),
        json!({
            "type": "remove",
            "target": "/slide[7]",
            "shift": null,
            "properties": {}
        }),
        json!({
            "type": "move",
            "target": "/slide[7]",
            "newParent": null,
            "position": { "type": "index", "index": 2 },
            "properties": {}
        }),
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["operations"] = json!([value["operations"][0].clone(), operation]);
        parse_and_validate(value).expect("one terminal whole-slide change is bounded and stable");
    }

    let structural = json!({
        "type": "remove",
        "target": "/slide[7]",
        "shift": null,
        "properties": {}
    });
    let element = valid_plan_value("/slide[1]/shape[@id=1]")["operations"][0].clone();
    for operations in [
        json!([structural.clone(), element.clone()]),
        json!([structural.clone(), structural.clone()]),
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["operations"] = operations;
        let error = parse_and_validate(value)
            .expect_err("whole-slide changes must be unique and final to avoid target drift");
        assert!(error.contains("at most one") || error.contains("must be last"));
    }

    for operation in [
        json!({
            "type": "move",
            "target": "/slide[7]",
            "newParent": null,
            "position": { "type": "after", "target": "/slide[6]" },
            "properties": {}
        }),
        json!({
            "type": "add",
            "parent": "/",
            "elementType": "slide",
            "copyFrom": "/slide[1]/shape[@id=1]",
            "position": null,
            "properties": {},
            "force": false
        }),
        json!({
            "type": "remove",
            "target": "/slide[7]",
            "shift": null,
            "properties": { "text": "escape" }
        }),
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["operations"] = json!([operation]);
        parse_and_validate(value).expect_err("unbounded whole-slide shapes must fail closed");
    }
}

#[test]
fn editor_plan_accepts_zero_based_element_z_order_but_rejects_invalid_indices() {
    let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
    value["operations"] = json!([{
        "type": "move",
        "target": "/slide[1]/shape[@id=1]",
        "newParent": null,
        "position": { "type": "index", "index": 0 },
        "properties": {}
    }]);
    parse_and_validate(value.clone()).expect("OfficeCLI z-order indices are zero-based");

    for index in [json!(-1), json!(1.5), json!("0")] {
        value["operations"][0]["position"]["index"] = index;
        parse_and_validate(value.clone()).expect_err("invalid z-order index must fail closed");
    }
}

#[test]
fn editor_plan_rejects_non_mutations_force_and_provider_escape_fields() {
    for operation in [
        json!({ "type": "validate" }),
        json!({ "type": "get", "target": "/slide[1]/shape[@id=1]", "depth": 1 }),
        json!({ "type": "query", "selector": "*", "contains": null, "compact": false, "fields": [] }),
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["operations"] = json!([operation]);
        let error = parse_and_validate(value).expect_err("read-only operation must be rejected");
        assert!(error.contains("non-mutation"), "unexpected error: {error}");
    }

    let mut forced = valid_plan_value("/slide[1]/shape[@id=1]");
    forced["operations"][0]["force"] = json!(true);
    let error = parse_and_validate(forced).expect_err("force must be rejected");
    assert!(error.contains("force"), "unexpected error: {error}");

    for (field, injected) in [
        ("argv", json!(["raw-set", "--best-effort"])),
        ("provider", json!("officecli")),
        ("raw", json!({ "command": "batch", "bestEffort": true })),
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["operations"][0][field] = injected;
        let error = parse_and_validate(value).expect_err("unknown provider field must be rejected");
        assert!(error.contains("deserialize"), "unexpected error: {error}");
    }
}

#[test]
fn editor_plan_allows_only_declared_picture_resources_and_officecli_chart_scalars() {
    let mut picture = valid_plan_value("/slide[2]/picture[@id=17]");
    picture["operations"][0]["properties"] = json!({
        "src": {
            "resourcePath": format!(
                "{}media/hero.png",
                crate::office::OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX
            )
        }
    });
    parse_and_validate(picture.clone()).expect("declared picture placeholder is supported");

    for properties in [
        json!({ "src": "/etc/passwd" }),
        json!({ "src": { "resourcePath": "/etc/passwd" } }),
        json!({ "preview": { "resourcePath": "media/hero.png" } }),
        json!({ "custom": { "nested": true } }),
    ] {
        picture["operations"][0]["properties"] = properties;
        parse_and_validate(picture.clone())
            .expect_err("undeclared paths and nested raw provider values must fail closed");
    }

    let mut chart = valid_plan_value("/slide[4]/chart[@id=11]");
    chart["operations"][0]["properties"] = json!({
        "categories": "Q1,Q2",
        "series1": "Revenue:12,18",
        "series2": "Margin:3.5,4.25"
    });
    parse_and_validate(chart.clone())
        .expect("SDK-compiled categories and contiguous seriesN scalars are supported");

    for properties in [
        json!({ "categories": "Q1,Q2", "series": "raw-json-provider-drift" }),
        json!({ "categories": "Q1,Q2", "series2": "Revenue:12,18" }),
        json!({ "categories": "Q1,Q2", "series1": "Revenue:12" }),
        json!({ "categories": "Q1", "series1": "Revenue:NaN" }),
    ] {
        chart["operations"][0]["properties"] = properties;
        parse_and_validate(chart.clone())
            .expect_err("malformed or provider-drifted chart encoding must fail closed");
    }

    let mut non_chart = valid_plan_value("/slide[4]/shape[@id=11]");
    non_chart["operations"][0]["properties"] = json!({
        "categories": "Q1",
        "series1": "Revenue:12"
    });
    parse_and_validate(non_chart).expect_err("chart data cannot be applied to a non-chart anchor");
}

#[test]
fn editor_plan_rejects_unsafe_source_destination_and_wrong_mode_or_schema() {
    for source in [
        "",
        "../source.pptx",
        "/tmp/source.pptx",
        "source.pdf",
        "source\0.pptx",
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["source"]["mountPath"] = json!(source);
        assert!(
            parse_and_validate(value).is_err(),
            "unsafe source `{source}` was accepted"
        );
    }

    for output in [
        "",
        "outputs/result.pdf",
        "outputs/result.pptx\nnext",
        "/tmp/result.pptx",
        "../result.pptx",
    ] {
        let mut value = valid_plan_value("/slide[1]/shape[@id=1]");
        value["destination"]["path"] = json!(output);
        assert!(
            parse_and_validate(value).is_err(),
            "unsafe output `{output}` was accepted"
        );
    }

    let mut same_source_and_output = valid_plan_value("/slide[1]/shape[@id=1]");
    same_source_and_output["destination"]["path"] = json!("source.pptx");
    assert!(
        parse_and_validate(same_source_and_output).is_err(),
        "save-as v1 must not accept the source mount path as its destination"
    );

    let mut wrong_mode = valid_plan_value("/slide[1]/shape[@id=1]");
    wrong_mode["mode"] = json!("inPlace");
    assert!(parse_and_validate(wrong_mode)
        .expect_err("in-place mode is not part of v1")
        .contains("deserialize"));

    let mut wrong_schema = valid_plan_value("/slide[1]/shape[@id=1]");
    wrong_schema["schemaVersion"] = json!(2);
    assert!(parse_and_validate(wrong_schema)
        .expect_err("unknown schema must fail closed")
        .contains("schema"));
}

#[test]
fn editor_plan_enforces_operation_and_file_size_limits() {
    let mut empty = valid_plan_value("/slide[1]/shape[@id=1]");
    empty["operations"] = json!([]);
    assert!(parse_and_validate(empty).is_err());

    let operation = valid_plan_value("/slide[1]/shape[@id=1]")["operations"][0].clone();
    let mut too_many = valid_plan_value("/slide[1]/shape[@id=1]");
    too_many["operations"] = Value::Array(vec![operation; MAX_PRESENTATION_EDITOR_OPERATIONS + 1]);
    assert!(parse_and_validate(too_many).is_err());

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("plan.json");
    fs::write(
        &path,
        vec![b' '; MAX_PRESENTATION_EDITOR_PLAN_BYTES as usize + 1],
    )
    .unwrap();
    let error = read_presentation_editor_plan(&path).expect_err("oversized plan must fail closed");
    assert!(error.contains("exceeds"), "unexpected error: {error}");
}

#[cfg(unix)]
#[test]
fn editor_plan_reader_rejects_symlink_plan_files() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target.json");
    let link = directory.path().join("plan.json");
    fs::write(
        &target,
        serde_json::to_vec(&valid_plan_value("/slide[1]/shape[@id=1]")).unwrap(),
    )
    .unwrap();
    symlink(&target, &link).unwrap();

    assert!(read_presentation_editor_plan(&link).is_err());
}
