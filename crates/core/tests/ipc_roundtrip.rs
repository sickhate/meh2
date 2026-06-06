//! Versioned IPC envelope round-trip over a Unix socket pair.

use meh_core::{
    IpcCmd, IpcResponse, ipc_read_reply, ipc_read_request, ipc_write_reply, ipc_write_request,
};

#[tokio::test]
async fn ping_roundtrip() {
    let (client, server) = tokio::net::UnixStream::pair().expect("socket pair");

    let client_task = tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(client);
        ipc_write_request(&mut writer, &IpcCmd::Ping)
            .await
            .expect("write request");
        ipc_read_reply(&mut reader).await.expect("read reply")
    });

    let (mut reader, mut writer) = tokio::io::split(server);
    let cmd = ipc_read_request(&mut reader)
        .await
        .expect("read request");
    assert!(matches!(cmd, IpcCmd::Ping));
    ipc_write_reply(&mut writer, &IpcResponse::ok("pong"))
        .await
        .expect("write reply");

    let resp = client_task.await.expect("client task");
    match resp {
        IpcResponse::Ok(msg) => assert_eq!(msg, "pong"),
        IpcResponse::Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn rejects_stale_protocol_version() {
    use meh_core::{IpcRequest, ipc_write};

    let (client, server) = tokio::net::UnixStream::pair().expect("socket pair");
    let (mut client_reader, _client_writer) = tokio::io::split(client);
    let (_server_reader, mut server_writer) = tokio::io::split(server);

    ipc_write(
        &mut server_writer,
        &IpcRequest {
            version: 0,
            cmd: IpcCmd::Ping,
        },
    )
    .await
    .expect("write stale request");

    let result = ipc_read_request(&mut client_reader).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("protocol mismatch")
    );
}
