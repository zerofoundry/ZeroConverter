//! Runs the real binary, the way scripts and other tools will.
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn zeroconverter(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zeroconverter"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run zeroconverter")
}

/// Every stdout line is a JSON object.
fn lines(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l} ({e})")))
        .collect()
}

fn png(path: &Path) {
    image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 40, 40, 255]))
        .save(path)
        .unwrap();
}

#[test]
fn version_is_json_with_the_interface_version() {
    let dir = tempfile::tempdir().unwrap();
    let out = zeroconverter(&["version", "--json"], dir.path());
    assert_eq!(out.status.code(), Some(0));
    let v = &lines(&out)[0];
    assert_eq!(v["tool"], "zeroconverter");
    assert_eq!(v["interface"], "0");
    assert_eq!(v["job_schema"], "zeroconverter.job/v0");
    assert_eq!(v["standard"], "zero-app/v0");
    assert!(!v["build"].as_str().unwrap().is_empty());
}

#[test]
fn capabilities_list_formats_and_routes() {
    let dir = tempfile::tempdir().unwrap();
    let out = zeroconverter(&["capabilities", "--json"], dir.path());
    assert_eq!(out.status.code(), Some(0));
    let caps = &lines(&out)[0];
    let image = caps["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "image")
        .unwrap();
    assert!(image["formats"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == "webp" && f["write"] == true));
    assert!(image["options"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["key"] == "resize_mode"));
    // Each writable format carries the full specs it honours; quality is
    // offered where the encoder uses it, not on PNG.
    let format = |id: &str| {
        image["formats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(format("jpg")["options"][0]["key"], "quality");
    assert_eq!(format("jpg")["options"][0]["default"], 90);
    assert!(format("png")["options"]
        .as_array()
        .unwrap()
        .iter()
        .all(|o| o["key"] != "quality"));
    assert_eq!(caps["job_schemas"][0], "zeroconverter.job/v0");
    assert_eq!(caps["effects"]["network"], "none");
}

#[test]
fn run_converts_and_reports_size_and_hash() {
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("red.png"));
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"zeroconverter.job/v0","inputs":["red.png"],"format":"webp"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all = lines(&out);
    assert_eq!(all[0]["event"], "job_started");
    assert_eq!(all[0]["schema"], "zeroconverter.event/v0");
    assert_eq!(all[0]["items"], 1);
    let result = all.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["summary"]["ok"], 1);
    let item = &result["items"][0];
    assert_eq!(item["status"], "ok");
    assert_eq!(item["output"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(item["output"]["rel_path"], "red.webp");
    assert!(dir.path().join("red.webp").exists());

    // Same input, same options, same version: same bytes.
    std::fs::remove_file(dir.path().join("red.webp")).unwrap();
    let again = lines(&zeroconverter(&["run", "--job", "job.json"], dir.path()));
    assert_eq!(
        again.last().unwrap()["items"][0]["output"]["sha256"],
        item["output"]["sha256"]
    );
}

#[test]
fn a_missing_input_fails_that_item_with_a_code() {
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("red.png"));
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"zeroconverter.job/v0","inputs":["red.png","nope.png"],"format":"bmp"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(1));
    let result = lines(&out).pop().unwrap();
    assert_eq!(result["summary"]["ok"], 1);
    assert_eq!(result["items"][1]["status"], "failed");
    assert_eq!(result["items"][1]["error"]["code"], "io");
    assert_eq!(result["items"][1]["retryable"], "unknown");
}

/// A format added to the image table reaches the command line on its own:
/// QOI is listed both ways, PNG → QOI is a route, and a run makes the file.
#[test]
fn a_table_format_reaches_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let caps = &lines(&zeroconverter(&["capabilities", "--json"], dir.path()))[0];
    let image = caps["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "image")
        .expect("image family");
    let text = image.to_string();
    assert!(text.contains("\"qoi\""), "qoi missing: {text}");

    png(&dir.path().join("red.png"));
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"zeroconverter.job/v0","inputs":["red.png"],"format":"qoi"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(0));
    let bytes = std::fs::read(dir.path().join("red.qoi")).unwrap();
    assert!(bytes.starts_with(b"qoif"));
}

/// The v0 event stream, in full: what a script may rely on. Fields may be
/// added later; none of these may go away or change meaning.
#[test]
fn the_v0_event_stream_keeps_its_shape() {
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("red.png"));
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"zeroconverter.job/v0","inputs":["red.png","nope.png"],"format":"webp"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(1), "one item failed");
    let all = lines(&out);
    for line in &all {
        assert!(line["event"].is_string(), "{line}");
    }
    assert_eq!(all.first().unwrap()["event"], "job_started");
    assert_eq!(all.first().unwrap()["schema"], "zeroconverter.event/v0");
    assert_eq!(all.first().unwrap()["items"], 2);
    let result = all.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["schema"], "zeroconverter.result/v0");
    assert_eq!(
        all.iter().filter(|l| l["event"] == "result").count(),
        1,
        "one result, last"
    );
    // One finished event per input, each naming its index.
    let finished: Vec<_> = all
        .iter()
        .filter(|l| l["event"] == "item_finished")
        .map(|l| l["item"]["index"].as_u64().expect("item.index"))
        .collect();
    let mut sorted = finished.clone();
    sorted.sort();
    assert_eq!(sorted, [0, 1], "{finished:?}");

    assert_eq!(result["summary"]["ok"], 1);
    assert_eq!(result["summary"]["failed"], 1);
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["status"], "ok");
    for key in ["rel_path", "bytes", "sha256"] {
        assert!(!items[0]["output"][key].is_null(), "output.{key}");
    }
    assert_eq!(items[1]["status"], "failed");
    for key in ["code", "message"] {
        assert!(items[1]["error"][key].is_string(), "error.{key}");
    }
    assert!(items[1]["retryable"].is_string());
}

/// Every option the capabilities list advertises, with each listed value and
/// its default, is accepted by the family that reads it.
#[test]
fn advertised_image_options_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("red.png"));
    let caps = &lines(&zeroconverter(&["capabilities", "--json"], dir.path()))[0];
    let image = caps["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "image")
        .unwrap();
    let options = image["options"].as_array().expect("image options");
    assert!(!options.is_empty());
    for option in options {
        let key = option["key"].as_str().unwrap();
        let format = option["formats"][0].as_str().unwrap_or("webp");
        let mut values: Vec<Value> = option["values"].as_array().cloned().unwrap_or_default();
        if !option["default"].is_null() {
            values.push(option["default"].clone());
        }
        if values.is_empty() {
            // A number without a list: any small positive value.
            values.push(serde_json::json!(4));
        }
        for value in values {
            let mut opts = serde_json::json!({ "resize_width": 4, "resize_height": 4 });
            opts[key] = value.clone();
            let job = serde_json::json!({
                "schema": "zeroconverter.job/v0",
                "inputs": ["red.png"],
                "format": format,
                "options": opts,
            });
            std::fs::write(dir.path().join("job.json"), job.to_string()).unwrap();
            let out = zeroconverter(&["validate", "--job", "job.json"], dir.path());
            assert_eq!(
                lines(&out)[0]["items"][0]["status"],
                "valid",
                "{key} = {value} for {format}: {}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }
}

#[test]
fn validate_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("red.png"));
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"zeroconverter.job/v0","inputs":["red.png"],"format":"jpg"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["validate", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(lines(&out)[0]["items"][0]["status"], "valid");
    assert!(!dir.path().join("red.jpg").exists());
}

#[test]
fn a_bad_job_file_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"other/v9","inputs":[],"format":"x"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(lines(&out)[0]["error"]["code"], "invalid_job");

    for job in [
        r#"{"schema":"zeroconverter.job/v0","inputs":["a.png"],"format":""}"#,
        r#"{"schema":"zeroconverter.job/v0","inputs":[""],"format":"webp"}"#,
    ] {
        std::fs::write(dir.path().join("job.json"), job).unwrap();
        let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
        assert_eq!(out.status.code(), Some(2), "{job}");
        assert_eq!(lines(&out)[0]["error"]["code"], "invalid_job", "{job}");
    }

    let unknown = zeroconverter(&["frobnicate"], dir.path());
    assert_eq!(unknown.status.code(), Some(2));
    assert!(unknown.stdout.is_empty());
}

#[test]
fn a_large_input_needs_consent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("big.wav"))
        .unwrap()
        .set_len(100 * 1024 * 1024 + 1)
        .unwrap();
    std::fs::write(
        dir.path().join("job.json"),
        r#"{"schema":"zeroconverter.job/v0","inputs":["big.wav"],"format":"flac"}"#,
    )
    .unwrap();
    let out = zeroconverter(&["validate", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(1));
    let item = &lines(&out)[0]["items"][0];
    assert_eq!(item["error"]["code"], "consent_required", "{item}");
    assert_eq!(item["retryable"], false);
}

/// docs/schema/zeroconverter.job.v0.json lists exactly the fields the CLI
/// accepts: a job with all of them is read, one more field is refused.
#[test]
fn the_job_schema_matches_the_job_file() {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/schema/zeroconverter.job.v0.json"
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(schema["additionalProperties"], false);
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("red.png"));
    let mut job = serde_json::json!({
        "schema": "zeroconverter.job/v0",
        "inputs": ["red.png"],
        "format": "webp",
        "output_dir": ".",
        "options": { "quality": 80 },
        "overwrite": false,
        "confirm_large": false,
    });
    let fields: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
    let mut sent: Vec<&String> = job.as_object().unwrap().keys().collect();
    let mut fields_sorted = fields.clone();
    fields_sorted.sort();
    sent.sort();
    assert_eq!(fields_sorted, sent, "update the schema or this test");
    for required in schema["required"].as_array().unwrap() {
        assert!(job.get(required.as_str().unwrap()).is_some());
    }

    let write = |job: &Value| std::fs::write(dir.path().join("job.json"), job.to_string()).unwrap();
    write(&job);
    let out = zeroconverter(&["validate", "--job", "job.json"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    job["extra"] = true.into();
    write(&job);
    let out = zeroconverter(&["validate", "--job", "job.json"], dir.path());
    assert_eq!(out.status.code(), Some(2));
}

/// Relative inputs and output_dir resolve once against the current folder
/// (not `out/out/...` or `textures/textures/...`).
#[test]
fn relative_paths_are_resolved_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("textures")).unwrap();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    png(&dir.path().join("textures").join("rock.png"));

    let job = r#"{"schema":"zeroconverter.job/v0","inputs":["textures/rock.png"],"format":"webp","output_dir":"out"}"#;
    std::fs::write(dir.path().join("job.json"), job).unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(dir.path().join("out").join("rock.webp").is_file());
    assert!(!dir.path().join("out").join("out").exists());
    assert_eq!(
        lines(&out).last().unwrap()["items"][0]["output"]["rel_path"],
        "rock.webp"
    );

    let job = r#"{"schema":"zeroconverter.job/v0","inputs":["textures/rock.png"],"format":"bmp"}"#;
    std::fs::write(dir.path().join("job.json"), job).unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(dir.path().join("textures").join("rock.bmp").is_file());
    assert!(!dir.path().join("textures").join("textures").exists());
}

/// With overwrite on, an output never replaces another input of the job.
#[test]
fn overwrite_keeps_the_jobs_other_inputs() {
    let dir = tempfile::tempdir().unwrap();
    png(&dir.path().join("a.png"));
    png(&dir.path().join("b.png"));
    std::fs::copy(dir.path().join("b.png"), dir.path().join("a.bmp")).unwrap();
    let before = std::fs::read(dir.path().join("a.bmp")).unwrap();
    // a.bmp is listed first so it is still an input when a.png is converted.
    let job = r#"{"schema":"zeroconverter.job/v0","inputs":["a.bmp","a.png"],"format":"bmp","overwrite":true}"#;
    std::fs::write(dir.path().join("job.json"), job).unwrap();
    let out = zeroconverter(&["run", "--job", "job.json"], dir.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(std::fs::read(dir.path().join("a.bmp")).unwrap(), before);
    let result = lines(&out).pop().unwrap();
    assert_ne!(result["items"][1]["output"]["rel_path"], "a.bmp");
}

#[test]
fn unknown_or_extra_arguments_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["version", "--yaml"][..],
        &["run"][..],
        &["run", "--job"][..],
        &["run", "--job", "a.json", "--job", "b.json"][..],
        &["capabilities", "--json", "extra"][..],
    ] {
        let out = zeroconverter(args, dir.path());
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}
