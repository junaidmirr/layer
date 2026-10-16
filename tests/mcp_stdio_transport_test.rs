use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn test_mcp_stdio_transport_resilience_after_bootstrap() {
    let temp_dir =
        std::env::temp_dir().join(format!("layer_mcp_transport_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let layer_bin = env!("CARGO_BIN_EXE_layer");
    let mut child = Command::new(layer_bin)
        .arg("mcp")
        .env("LAYER_AUTO_APPROVE", "1")
        .current_dir(&temp_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn layer mcp binary");

    let mut stdin = child.stdin.take().expect("Failed to open child stdin");
    let stdout = child.stdout.take().expect("Failed to open child stdout");
    let mut reader = BufReader::new(stdout);

    // Helper closure to write a JSON-RPC request line
    let write_req = |stdin: &mut std::process::ChildStdin, val: &Value| {
        let serialized = serde_json::to_string(val).unwrap();
        stdin.write_all(serialized.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };

    // Helper closure to read one JSON-RPC response line
    let read_resp = |reader: &mut BufReader<std::process::ChildStdout>| -> Value {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .expect("Failed to read line from MCP server");
        assert!(
            bytes_read > 0,
            "MCP server closed transport unexpectedly (EOF)!"
        );
        serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("Failed to parse JSON response '{}': {}", line.trim(), e))
    };

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "rootUri": format!("file://{}", temp_dir.display()),
            "capabilities": {}
        }
    });
    write_req(&mut stdin, &init_req);
    let init_resp = read_resp(&mut reader);
    assert_eq!(init_resp["id"], 1);
    assert_eq!(
        init_resp["result"]["serverInfo"]["name"],
        "layer-context-engine"
    );

    // 2. Notification initialized
    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    write_req(&mut stdin, &notif);

    // 3. Fast pure Rust bootstrap_project (<100ms)
    let bootstrap_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "bootstrap_project",
            "arguments": {
                "preset": "nextjs",
                "project_name": "light_circuit_studio",
                "packages": ["three", "@types/three"]
            }
        }
    });
    let start_time = Instant::now();
    write_req(&mut stdin, &bootstrap_req);
    let bootstrap_resp = read_resp(&mut reader);
    let bootstrap_elapsed = start_time.elapsed();

    assert_eq!(bootstrap_resp["id"], 2);
    let content_text = bootstrap_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(content_text.contains("light_circuit_studio"));
    assert!(
        bootstrap_elapsed < Duration::from_millis(500),
        "bootstrap_project should complete in <500ms, took {:?}",
        bootstrap_elapsed
    );

    let project_dir = temp_dir.join("light_circuit_studio");
    assert!(project_dir.join("package.json").exists());
    assert!(project_dir.join("app/page.tsx").exists());

    // 4. Send rapid batch: 3x inspect_package_api + 3x get_lines back-to-back WITHOUT waiting
    let batch_requests = vec![
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "inspect_package_api",
                "arguments": { "package_name": "three" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "inspect_package_api",
                "arguments": { "package_name": "@types/three" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "inspect_package_api",
                "arguments": { "package_name": "non_existent_pkg" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "get_lines",
                "arguments": {
                    "file_path": "package.json",
                    "start_line": 1,
                    "end_line": 10
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "get_lines",
                "arguments": {
                    "file_path": "app/page.tsx",
                    "start_line": 1,
                    "end_line": 15
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "get_lines",
                "arguments": {
                    "file_path": "tsconfig.json",
                    "start_line": 1,
                    "end_line": 10
                }
            }
        }),
    ];

    // Pipeline all 6 requests into stdin at once
    for req in &batch_requests {
        let serialized = serde_json::to_string(req).unwrap();
        stdin.write_all(serialized.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    stdin.flush().unwrap();

    // Collect all 6 responses
    for expected_id in 3..=8 {
        let resp = read_resp(&mut reader);
        assert_eq!(
            resp["id"], expected_id,
            "Response ID mismatch during pipelined batch execution"
        );
        assert!(
            resp.get("result").is_some(),
            "Pipelined tool call ID {} failed: {:?}",
            expected_id,
            resp
        );
    }

    // 5. Test Content-Length framing support
    let cl_req = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "get_lines",
            "arguments": {
                "file_path": "package.json",
                "start_line": 1,
                "end_line": 5
            }
        }
    });
    let cl_body = serde_json::to_string(&cl_req).unwrap();
    let header_msg = format!("Content-Length: {}\r\n\r\n{}", cl_body.len(), cl_body);
    stdin.write_all(header_msg.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let cl_resp = read_resp(&mut reader);
    assert_eq!(cl_resp["id"], 9);
    assert!(cl_resp.get("result").is_some());

    // 6. Test JSON-RPC Array batch request
    let array_batch = json!([
        {
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "name": "get_lines",
                "arguments": {
                    "file_path": "package.json",
                    "start_line": 1,
                    "end_line": 3
                }
            }
        },
        {
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "get_lines",
                "arguments": {
                    "file_path": "package.json",
                    "start_line": 4,
                    "end_line": 6
                }
            }
        }
    ]);
    let array_serialized = serde_json::to_string(&array_batch).unwrap();
    stdin.write_all(array_serialized.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();

    let b1 = read_resp(&mut reader);
    assert_eq!(b1["id"], 10);
    let b2 = read_resp(&mut reader);
    assert_eq!(b2["id"], 11);

    // 7. Clean Shutdown
    let shutdown_req = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "shutdown"
    });
    write_req(&mut stdin, &shutdown_req);
    let shutdown_resp = read_resp(&mut reader);
    assert_eq!(shutdown_resp["id"], 12);

    let exit_req = json!({
        "jsonrpc": "2.0",
        "method": "exit"
    });
    write_req(&mut stdin, &exit_req);

    let status = child.wait().expect("Child process failed to exit cleanly");
    assert!(
        status.success(),
        "Layer server exited with non-zero exit code: {:?}",
        status
    );

    let _ = fs::remove_dir_all(&temp_dir);
}
