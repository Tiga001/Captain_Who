use super::discovery::discover_with_test_path;
use super::execution::{compile_office_arguments, install_commit_test_hook, CommitTestPhase};
use super::types::office_agent_input_placeholder;
use super::*;
use crate::artifact_runtime::{ArtifactRuntimeInvocation, ArtifactRuntimeKind};
use crate::file_input::{
    prepare_agent_file_input_bindings, read_verified_agent_file_input,
    AgentFileInputExecutionContext,
};
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentCancellationToken,
    AgentFileInputRef, AgentFileInputSpec, AgentInputAttachmentKind, AgentPermissions,
    AgentReadPermission, AgentWritePermission,
};
use lopdf::dictionary;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

struct Fixture {
    workspace: tempfile::TempDir,
    engine_dir: tempfile::TempDir,
    _proxy_dir: tempfile::TempDir,
    _render_runtime_dir: tempfile::TempDir,
    _word_pdf_runtime_dir: tempfile::TempDir,
    engine: OfficeCliEngine,
}

impl Fixture {
    fn new(script: &str) -> Self {
        Self::new_with_word_pdf_renderer(script, b"#!/bin/sh\necho 'LibreOffice 26.2.4.2'\n")
    }

    fn new_with_word_pdf_renderer(script: &str, renderer: &[u8]) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let engine_dir = tempfile::tempdir().unwrap();
        let executable = engine_dir.path().join("officecli");
        write_executable(&executable, &fixture_officecli_script(script));
        let proxy_dir = tempfile::tempdir().unwrap();
        let proxy = proxy_dir.path().join("core-server");
        write_executable(
            &proxy,
            "#!/bin/sh\nprintf 'mycopilot-office-browser-proxy-v1\\n'\n",
        );
        let render_runtime_dir = tempfile::tempdir().unwrap();
        super::render_runtime::write_test_render_runtime(render_runtime_dir.path());
        let word_pdf_runtime_dir = tempfile::tempdir().unwrap();
        super::word_pdf_render_runtime::write_test_word_pdf_render_runtime_with_executable(
            word_pdf_runtime_dir.path(),
            renderer,
        );
        let options = OfficeCliDiscoveryOptions::new()
            .with_configured_executable(&executable)
            .with_configured_render_runtime_dir(render_runtime_dir.path())
            .with_configured_word_pdf_render_runtime_dir(word_pdf_runtime_dir.path())
            .with_browser_proxy_executable(&proxy)
            .with_workspace_root(workspace.path());
        let engine = OfficeCliEngine::discover(&options).unwrap();
        Self {
            workspace,
            engine_dir,
            _proxy_dir: proxy_dir,
            _render_runtime_dir: render_runtime_dir,
            _word_pdf_runtime_dir: word_pdf_runtime_dir,
            engine,
        }
    }

    fn request(&self, operation: OfficeOperation) -> OfficeExecutionRequest {
        OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Document,
            operation,
            document_path: Some("sample.docx".to_string()),
            parameters: default_parameters(operation),
            output_path: None,
            destination_path: None,
            inputs: Vec::new(),
            timeout_ms: Some(10_000),
        }
    }
}

fn fixture_officecli_script(script: &str) -> String {
    format!(
        "#!/bin/sh\nif [ -n \"$MYCOPILOT_OFFICE_BROWSER_FAILURE_MARKER\" ] && [ -n \"$MYCOPILOT_OFFICE_BROWSER_MARKER_NONCE\" ]; then\n  printf '{{\"schemaVersion\":1,\"nonce\":\"%s\",\"status\":\"success\",\"invocationCount\":1,\"maxInvocations\":%s,\"timedOut\":false}}' \"$MYCOPILOT_OFFICE_BROWSER_MARKER_NONCE\" \"$MYCOPILOT_OFFICE_BROWSER_MAX_INVOCATIONS\" > \"$MYCOPILOT_OFFICE_BROWSER_FAILURE_MARKER\"\nfi\n{script}"
    )
}

fn default_parameters(operation: OfficeOperation) -> OfficeOperationParameters {
    match operation {
        OfficeOperation::Help => OfficeOperationParameters::Help {
            verb: None,
            element: None,
        },
        OfficeOperation::Create => OfficeOperationParameters::Create {
            locale: None,
            minimal: false,
            overwrite: false,
        },
        OfficeOperation::View => OfficeOperationParameters::View {
            mode: OfficeViewMode::Text,
            start: None,
            end: None,
            max_lines: None,
            issue_type: None,
            limit: None,
            columns: Vec::new(),
            pages: Vec::new(),
            range: None,
            viewport: None,
            grid: None,
            render_mode: None,
            page_count: false,
        },
        OfficeOperation::Get => OfficeOperationParameters::Get {
            target: None,
            depth: None,
        },
        OfficeOperation::Query => OfficeOperationParameters::Query {
            selector: "*".to_string(),
            contains: None,
            compact: false,
            fields: Vec::new(),
        },
        OfficeOperation::Validate => OfficeOperationParameters::Validate,
        OfficeOperation::Set => OfficeOperationParameters::Set {
            target: "/body".to_string(),
            properties: [("text".to_string(), serde_json::json!("updated"))]
                .into_iter()
                .collect(),
            replacement: None,
            force: false,
        },
        OfficeOperation::Add => OfficeOperationParameters::Add {
            parent: "/body".to_string(),
            element_type: "paragraph".to_string(),
            copy_from: None,
            position: None,
            properties: std::collections::BTreeMap::new(),
            force: false,
        },
        OfficeOperation::Remove => OfficeOperationParameters::Remove {
            target: "/body/p[1]".to_string(),
            shift: None,
            properties: std::collections::BTreeMap::new(),
        },
        OfficeOperation::Move => OfficeOperationParameters::Move {
            target: "/body/p[1]".to_string(),
            new_parent: None,
            position: Some(OfficeElementPosition::Index { index: 0 }),
            properties: std::collections::BTreeMap::new(),
        },
        OfficeOperation::Swap => OfficeOperationParameters::Swap {
            first_target: "/body/p[1]".to_string(),
            second_target: "/body/p[2]".to_string(),
        },
    }
}

fn workspace_context(path: &Path) -> OfficeExecutionContext {
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        ..AgentPermissions::default()
    };
    OfficeExecutionContext::new(Some(path.to_path_buf()), permissions, None)
}

fn screenshot_request(fixture: &Fixture, output_path: &str) -> OfficeExecutionRequest {
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Screenshot,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    request.output_path = Some(output_path.to_string());
    request
}

fn word_pdf_request(fixture: &Fixture, output_path: &str) -> OfficeExecutionRequest {
    let mut request = fixture.request(OfficeOperation::View);
    request.parameters = OfficeOperationParameters::View {
        mode: OfficeViewMode::Pdf,
        start: None,
        end: None,
        max_lines: None,
        issue_type: None,
        limit: None,
        columns: Vec::new(),
        pages: Vec::new(),
        range: None,
        viewport: None,
        grid: None,
        render_mode: None,
        page_count: false,
    };
    request.output_path = Some(output_path.to_string());
    request
}

fn permission_context(
    workspace: Option<&Path>,
    read: AgentReadPermission,
    write: AgentWritePermission,
) -> OfficeExecutionContext {
    let permissions = AgentPermissions {
        read,
        write,
        ..AgentPermissions::default()
    };
    OfficeExecutionContext::new(workspace.map(Path::to_path_buf), permissions, None)
}

fn prepared_path<'a>(
    prepared: &'a OfficePreparedExecution,
    slot: &OfficePathSlot,
) -> &'a OfficeFrozenPath {
    prepared
        .paths
        .iter()
        .find(|path| &path.slot == slot)
        .expect("prepared Office path slot")
}

fn write_executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

fn basic_script() -> &'static str {
    "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'OfficeCLI 1.2.3\\n'; exit 0; fi\nprintf 'stdout:%s:%s\\n' \"$1\" \"$2\"\nprintf 'stderr:%s\\n' \"$3\" >&2\nexit \"${OFFICECLI_TEST_EXIT:-0}\"\n"
}

fn write_docx(path: &Path, text: &str) {
    let file = fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive
        .write_all(
            b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>",
        )
        .unwrap();
    archive.start_file("word/document.xml", options).unwrap();
    archive.write_all(text.as_bytes()).unwrap();
    archive.finish().unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_renderable_word_pdf_smoke_docx(path: &Path, page_count: u32) {
    assert!(page_count > 0);
    let file = fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    let mut document = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
"#,
    );
    for page in 1..=page_count {
        document.push_str(&format!(
            "    <w:p><w:r><w:t>Managed Host transaction page {page}</w:t></w:r></w:p>\n"
        ));
        if page < page_count {
            document.push_str("    <w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>\n");
        }
    }
    document.push_str(
        r#"    <w:sectPr>
      <w:footerReference w:type="default" r:id="rId1"/>
      <w:pgSz w:w="11906" w:h="16838"/>
      <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>
    </w:sectPr>
  </w:body>
</w:document>"#,
    );
    let footer = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p>
    <w:r><w:t xml:space="preserve">Page </w:t></w:r>
    <w:r><w:fldChar w:fldCharType="begin"/></w:r>
    <w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>
    <w:r><w:fldChar w:fldCharType="separate"/></w:r>
    <w:r><w:t>1</w:t></w:r>
    <w:r><w:fldChar w:fldCharType="end"/></w:r>
    <w:r><w:t xml:space="preserve"> / </w:t></w:r>
    <w:r><w:fldChar w:fldCharType="begin"/></w:r>
    <w:r><w:instrText xml:space="preserve"> NUMPAGES </w:instrText></w:r>
    <w:r><w:fldChar w:fldCharType="separate"/></w:r>
    <w:r><w:t>{page_count}</w:t></w:r>
    <w:r><w:fldChar w:fldCharType="end"/></w:r>
  </w:p>
</w:ftr>"#
    );
    for (name, contents) in [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>
</Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
        ),
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>
</Relationships>"#,
        ),
        ("word/document.xml", document.as_str()),
        ("word/footer1.xml", footer.as_str()),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(contents.as_bytes()).unwrap();
    }
    archive.finish().unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn managed_artifact_runtime_paths() -> Option<(PathBuf, PathBuf)> {
    let runtime_root = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(".cache/artifact-runtime/current")
        });
    let receipt_path = runtime_root.join("component-receipt.json");
    if !receipt_path.is_file() {
        return None;
    }
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
    let relative_path = |pointer: &str| {
        let relative = receipt
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap();
        relative
            .split('/')
            .fold(runtime_root.clone(), |path, part| path.join(part))
    };
    let python = relative_path("/runtimes/python/executable");
    let pdf_cli = relative_path("/tools/pdfCli/path");
    Some((python, pdf_cli))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_managed_pdf_skill_page_coverage(pdf_path: &Path, expected_pages: u32) {
    let (python, pdf_cli) = managed_artifact_runtime_paths()
        .expect("real Word PDF acceptance requires MYCOPILOT_ARTIFACT_RUNTIME_DIR");
    let run = tempfile::tempdir().unwrap();
    let input = run.path().join("input.pdf");
    fs::copy(pdf_path, &input).unwrap();
    fs::create_dir(run.path().join("outputs")).unwrap();
    let invoke = |arguments: &[String]| {
        let mut command = std::process::Command::new(&python);
        command
            .arg(&pdf_cli)
            .args(arguments)
            .current_dir(run.path());
        command.env_clear();
        command.env("HOME", run.path());
        command.env("TMPDIR", run.path());
        command.env("TEMP", run.path());
        command.env("TMP", run.path());
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command.output().unwrap()
    };

    let info = invoke(&["pdfinfo".to_string(), "input.pdf".to_string()]);
    assert!(
        info.status.success(),
        "{}",
        String::from_utf8_lossy(&info.stderr)
    );
    let info = String::from_utf8(info.stdout).unwrap();
    assert!(info.contains("Encrypted: no"), "{info}");
    let pdfinfo_pages = info
        .lines()
        .find_map(|line| line.strip_prefix("Pages: "))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert_eq!(pdfinfo_pages, expected_pages);

    if expected_pages > 32 {
        let oversized = invoke(&[
            "pdftoppm".to_string(),
            "-f".to_string(),
            "1".to_string(),
            "-l".to_string(),
            expected_pages.to_string(),
            "-r".to_string(),
            "36".to_string(),
            "-png".to_string(),
            "input.pdf".to_string(),
            "outputs/oversized".to_string(),
        ]);
        assert!(!oversized.status.success());
        assert!(
            String::from_utf8_lossy(&oversized.stderr).contains("at most 32 pages"),
            "{}",
            String::from_utf8_lossy(&oversized.stderr)
        );
    }

    let mut ledger_entries = 0_u32;
    let mut first = 1_u32;
    while first <= expected_pages {
        let last = expected_pages.min(first + 31);
        let rendered = invoke(&[
            "pdftoppm".to_string(),
            "-f".to_string(),
            first.to_string(),
            "-l".to_string(),
            last.to_string(),
            "-r".to_string(),
            "36".to_string(),
            "-png".to_string(),
            "input.pdf".to_string(),
            "outputs/page".to_string(),
        ]);
        assert!(
            rendered.status.success(),
            "{}",
            String::from_utf8_lossy(&rendered.stderr)
        );
        ledger_entries += String::from_utf8(rendered.stdout)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("Generated outputs/page-"))
            .count() as u32;
        first = last + 1;
    }

    let page_images = fs::read_dir(run.path().join("outputs"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|value| value == "png"))
        .collect::<Vec<_>>();
    for image in &page_images {
        let decoded = image::ImageReader::open(image.path())
            .unwrap()
            .decode()
            .unwrap();
        assert!(decoded.width() > 0 && decoded.height() > 0);
    }
    assert_eq!(page_images.len() as u32, pdfinfo_pages);
    assert_eq!(ledger_entries, pdfinfo_pages);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_complex_word_pdf_acceptance_docx(path: &Path) {
    let (python, _) = managed_artifact_runtime_paths()
        .expect("complex Word PDF acceptance requires MYCOPILOT_ARTIFACT_RUNTIME_DIR");
    let run = tempfile::tempdir().unwrap();
    let script = run.path().join("build-complex-docx.py");
    fs::write(
        &script,
        r#"from pathlib import Path
import sys

from docx import Document
from docx.enum.section import WD_ORIENT, WD_SECTION_START
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Pt


def set_numbering(section, fmt, start):
    sect_pr = section._sectPr
    for existing in list(sect_pr.findall(qn("w:pgNumType"))):
        sect_pr.remove(existing)
    page_numbering = OxmlElement("w:pgNumType")
    page_numbering.set(qn("w:fmt"), fmt)
    page_numbering.set(qn("w:start"), str(start))
    sect_pr.append(page_numbering)


def append_field(paragraph, instruction, fallback):
    run = paragraph.add_run()
    begin = OxmlElement("w:fldChar")
    begin.set(qn("w:fldCharType"), "begin")
    instruction_element = OxmlElement("w:instrText")
    instruction_element.set(qn("xml:space"), "preserve")
    instruction_element.text = f" {instruction} "
    separate = OxmlElement("w:fldChar")
    separate.set(qn("w:fldCharType"), "separate")
    value = OxmlElement("w:t")
    value.text = fallback
    end = OxmlElement("w:fldChar")
    end.set(qn("w:fldCharType"), "end")
    for element in (begin, instruction_element, separate, value, end):
        run._r.append(element)


def install_footer(section):
    footer = section.footer
    footer.is_linked_to_previous = False
    paragraph = footer.paragraphs[0]
    for child in list(paragraph._p):
        paragraph._p.remove(child)
    paragraph.add_run("Page ")
    append_field(paragraph, "PAGE", "1")
    paragraph.add_run(" / ")
    append_field(paragraph, "NUMPAGES", "1")


document = Document()
first = document.sections[0]
set_numbering(first, "lowerRoman", 1)
install_footer(first)
document.add_heading("Roman-numbered portrait section", level=1)
document.add_paragraph("Roman section marker")

landscape = document.add_section(WD_SECTION_START.NEW_PAGE)
landscape.orientation = WD_ORIENT.LANDSCAPE
landscape.page_width, landscape.page_height = landscape.page_height, landscape.page_width
set_numbering(landscape, "decimal", 1)
install_footer(landscape)
document.add_heading("Landscape table section", level=1)
document.add_paragraph("Landscape section marker")
table = document.add_table(rows=1, cols=2)
table.rows[0].cells[0].text = "Index"
table.rows[0].cells[1].text = "Cross-page table content"
for index in range(1, 181):
    cells = table.add_row().cells
    cells[0].text = str(index)
    cells[1].text = f"Cross-page table row {index} with stable acceptance text"
    for paragraph in cells[1].paragraphs:
        for run in paragraph.runs:
            run.font.size = Pt(8)

last = document.add_section(WD_SECTION_START.NEW_PAGE)
set_numbering(last, "upperRoman", 1)
install_footer(last)
document.add_paragraph("After landscape marker")
document.add_page_break()
document.add_paragraph("")

document.save(Path(sys.argv[1]))
"#,
    )
    .unwrap();
    let output = std::process::Command::new(python)
        .arg(script)
        .arg(path)
        .current_dir(run.path())
        .env_clear()
        .env("HOME", run.path())
        .env("TMPDIR", run.path())
        .env("TEMP", run.path())
        .env("TMP", run.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_xlsx_package(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive
        .write_all(
            b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>",
        )
        .unwrap();
    archive.start_file("xl/workbook.xml", options).unwrap();
    archive
        .write_all(
            b"<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"/>",
        )
        .unwrap();
    archive.finish().unwrap();
}

fn fake_python_invocation(executable: PathBuf) -> ArtifactRuntimeInvocation {
    ArtifactRuntimeInvocation::new(
        "test-bundle".to_string(),
        "test-revision".to_string(),
        "test-fingerprint".to_string(),
        ArtifactRuntimeKind::Python,
        "3.12".to_string(),
        executable,
        Vec::new(),
        BTreeMap::new(),
    )
}

fn write_png(path: &Path, width: u32, height: u32) {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([24, 48, 72, 255]));
    image
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

fn write_pdf(path: &Path, page_count: u32) {
    let mut document = lopdf::Document::with_version("1.7");
    let pages_id = document.new_object_id();
    let page_ids = (0..page_count)
        .map(|_| {
            document.add_object(lopdf::dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            })
        })
        .collect::<Vec<_>>();
    document.objects.insert(
        pages_id,
        lopdf::Object::Dictionary(lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids.iter().copied().map(lopdf::Object::Reference).collect::<Vec<_>>(),
            "Count" => i64::from(page_count),
        }),
    );
    let catalog_id = document.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    document.save(path).unwrap();
}

fn copy_render_script(path: &Path) -> String {
    format!(
        "#!/bin/sh\nfor output in \"$@\"; do :; done\nprintf 'render-output:%s\\ncwd:%s\\n' \"$output\" \"$PWD\"\n/bin/cp '{}' \"$output\"\n",
        path.display()
    )
}

fn write_pptx(path: &Path, slide_count: u32) {
    let file = fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive.write_all(b"<Types/>").unwrap();
    archive.start_file("ppt/presentation.xml", options).unwrap();
    let slide_ids = (1..=slide_count)
        .map(|slide| format!("<p:sldId id=\"{}\" r:id=\"rId{slide}\"/>", 255 + slide))
        .collect::<String>();
    archive
        .write_all(format!("<p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><p:sldIdLst>{slide_ids}</p:sldIdLst><p:sldSz cx=\"12192000\" cy=\"6858000\"/></p:presentation>").as_bytes())
        .unwrap();
    archive
        .start_file("ppt/_rels/presentation.xml.rels", options)
        .unwrap();
    let relationships = (1..=slide_count)
        .map(|slide| format!("<Relationship Id=\"rId{slide}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{slide}.xml\"/>"))
        .collect::<String>();
    archive.write_all(format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>").as_bytes()).unwrap();
    for slide in 1..=slide_count {
        archive
            .start_file(format!("ppt/slides/slide{slide}.xml"), options)
            .unwrap();
        archive.write_all(b"<p:sld xmlns:p=\"urn:test\"/>").unwrap();
    }
    archive.finish().unwrap();
}

fn frozen_presentation_edit_request(
    fixture: &Fixture,
    operations: Vec<OfficeOperationParameters>,
) -> OfficePresentationEditRequest {
    let source_path = fixture.workspace.path().join("source.pptx");
    write_pptx(&source_path, 2);
    let source_spec = AgentFileInputSpec {
        mount_path: "source.pptx".to_string(),
        source: AgentFileInputRef::Workspace {
            path: "source.pptx".to_string(),
        },
    };
    let bindings = prepare_agent_file_input_bindings(
        Some(fixture.workspace.path()),
        AgentPermissions::default(),
        &AgentFileInputExecutionContext::default(),
        std::slice::from_ref(&source_spec),
        None,
    )
    .unwrap();
    let destination_binding = prepare_managed_script_binding(
        &workspace_context(fixture.workspace.path()),
        OfficeDocumentKind::Presentation,
        OfficeManagedScriptPurpose::EditPresentationPlan,
        "__mycopilot/presentation-editor/editor.mjs".to_string(),
        Some("source.pptx".to_string()),
        "edited.pptx",
    )
    .unwrap();
    OfficePresentationEditRequest {
        source_path: "source.pptx".to_string(),
        source_binding: bindings.into_iter().next().unwrap(),
        destination_path: "edited.pptx".to_string(),
        destination_binding,
        inputs: Vec::new(),
        input_bindings: Vec::new(),
        operations,
        timeout_ms: Some(10_000),
    }
}

fn presentation_text_set(target: &str, text: &str) -> OfficeOperationParameters {
    OfficeOperationParameters::Set {
        target: target.to_string(),
        properties: [("text".to_string(), serde_json::json!(text))]
            .into_iter()
            .collect(),
        replacement: None,
        force: false,
    }
}

mod engine_and_discovery;
mod managed_scripts;
mod permissions_and_paths;
mod presentation;
