use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn server(response: &'static [u8]) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(response).await.unwrap();
        String::from_utf8(request).unwrap().to_lowercase()
    });
    (url, task)
}

fn download(url: String, mirrors: Vec<String>) -> OcrDownload {
    OcrDownload {
        name: "test.onnx",
        url,
        mirrors,
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap()
}

#[tokio::test]
async fn failed_or_corrupt_source_falls_back_to_verified_mirror() {
    for response in [
        &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nxyz"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nab"[..],
    ] {
        let dir = super::super::asset::tests::temp_dir("mirror");
        let part = dir.join("model.part");
        let (primary, first) = server(response).await;
        let (mirror, second) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").await;
        let file = download(primary, vec![mirror]);
        download_verified(
            &client(),
            &file,
            &part,
            0,
            3,
            &AtomicBool::new(false),
            &|_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), b"abc");
        first.await.unwrap();
        assert!(!second.await.unwrap().contains("range:"));
        assert!(!is_installed(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn partial_download_resumes_across_sources_or_restarts_when_range_is_ignored() {
    for response in [
        &b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 1-2/3\r\nContent-Length: 2\r\n\r\nbc"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"[..],
    ] {
        let dir = super::super::asset::tests::temp_dir("resume");
        let part = dir.join("model.part");
        let (primary, first) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\na").await;
        let (mirror, second) = server(response).await;
        let file = download(primary, vec![mirror]);
        download_verified(&client(), &file, &part, 0, 3, &AtomicBool::new(false), &|_| {}).await.unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), b"abc");
        first.await.unwrap();
        assert!(second.await.unwrap().contains("range: bytes=1-"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn all_sources_failing_cannot_mark_installed() {
    let dir = super::super::asset::tests::temp_dir("failed-mirror");
    let part = dir.join("model.part");
    let (primary, first) = server(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
    let (mirror, second) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nxyz").await;
    let error = download_verified(
        &client(),
        &download(primary, vec![mirror]),
        &part,
        0,
        3,
        &AtomicBool::new(false),
        &|_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "ocr_download_failed");
    assert!(error.message.contains("403"));
    assert!(error.message.contains("校验失败"));
    assert!(!part.exists());
    assert!(!marker_path(&dir).exists());
    first.await.unwrap();
    second.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn cancellation_interrupts_headers_and_stalled_body_without_using_mirror() {
    for headers in [false, true] {
        let dir = super::super::asset::tests::temp_dir("cancel-mirror");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mirror = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let file = download(
            format!("http://{}/model", listener.local_addr().unwrap()),
            vec![format!("http://{}/model", mirror.local_addr().unwrap())],
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            socket.read(&mut request).await.unwrap();
            if headers {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\na")
                    .await
                    .unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            flag.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            download_verified(
                &client(),
                &file,
                &dir.join("model.part"),
                0,
                3,
                &cancel,
                &|_| {},
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "ocr_install_cancelled");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), mirror.accept())
                .await
                .is_err()
        );
        task.abort();
        let _ = task.await;
        assert!(!is_installed(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn invalid_resume_ranges_are_rejected() {
    assert!(valid_range(Some("bytes 1-2/3"), 1, 3));
    for range in [
        None,
        Some("bytes 0-1/3"),
        Some("bytes 1-1/3"),
        Some("bytes 1-2/*"),
        Some("bytes 1-2/4"),
        Some("invalid"),
    ] {
        assert!(!valid_range(range, 1, 3));
    }
}
