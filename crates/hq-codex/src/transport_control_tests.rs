#![cfg(unix)]

use std::{
    error::Error,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::transport::{JsonlTransport, TransportRead};

#[test]
fn interrupt_response_bypasses_an_unfinished_ordinary_rpc() -> Result<(), Box<dyn Error>> {
    let (client, mut server) = UnixStream::pair()?;
    server.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut transport = JsonlTransport::start(Box::new(client.try_clone()?), 8)?;
    transport.bind_input(Box::new(client))?;
    let control = transport.control();
    let (release, released) = mpsc::channel();
    let provider = thread::spawn(move || -> Result<(), String> {
        let mut reader = BufReader::new(server.try_clone().map_err(|error| error.to_string())?);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let original: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let interrupt: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if interrupt["method"] != "turn/interrupt" || interrupt["params"]["turnId"] != "turn-active"
        {
            return Err("unexpected control target".to_owned());
        }
        writeln!(server, "{}", json!({"id": interrupt["id"], "result": {}}))
            .map_err(|error| error.to_string())?;
        released
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        writeln!(
            server,
            "{}",
            json!({"id": original["id"], "result": {"original": true}})
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    });
    transport.write(&json!({"id": 7, "method": "thread/read", "params": {}}))?;
    let response = control.request(
        "turn/interrupt",
        json!({"threadId": "thread-active", "turnId": "turn-active"}),
        Duration::from_secs(1),
    )?;
    assert_eq!(response.result, Some(json!({})));
    release.send(())?;
    let TransportRead::Message(original) = transport.receive(Duration::from_secs(1)) else {
        return Err("ordinary response was not preserved".into());
    };
    assert_eq!(original.id, Some(json!(7)));
    assert_eq!(original.result, Some(json!({"original": true})));
    provider.join().map_err(|_| "provider panicked")??;
    transport.close_input();
    transport.join_reader()?;
    Ok(())
}

#[test]
fn late_control_response_cannot_satisfy_an_ordinary_request() -> Result<(), Box<dyn Error>> {
    let (client, mut server) = UnixStream::pair()?;
    server.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut transport = JsonlTransport::start(Box::new(client.try_clone()?), 8)?;
    transport.bind_input(Box::new(client))?;
    let response =
        transport
            .control()
            .request("turn/interrupt", json!({}), Duration::from_millis(1));
    assert!(matches!(
        response,
        Err(hq_harness::HarnessError {
            class: hq_harness::HarnessErrorClass::Unavailable,
            ..
        })
    ));
    let mut reader = BufReader::new(server.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let request: Value = serde_json::from_str(&line)?;
    writeln!(
        server,
        "{}",
        json!({"id": request["id"], "result": {"late": true}})
    )?;
    writeln!(
        server,
        "{}",
        json!({"id": 33, "result": {"ordinary": true}})
    )?;
    let TransportRead::Message(response) = transport.receive(Duration::from_secs(1)) else {
        return Err("ordinary response missing".into());
    };
    assert_eq!(response.id, Some(json!(33)));
    drop(reader);
    drop(server);
    transport.join_reader()?;
    assert!(matches!(
        transport
            .control()
            .request("turn/interrupt", json!({}), Duration::from_secs(1)),
        Err(hq_harness::HarnessError {
            class: hq_harness::HarnessErrorClass::TransportClosed,
            ..
        })
    ));
    Ok(())
}
