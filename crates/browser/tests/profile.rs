use std::time::{Duration, SystemTime, UNIX_EPOCH};

use browser::{Browser, Profile, Renderers};
use url::Url;

fn temp_data_home() -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tinybrowser-profile-{stamp}"));
    std::fs::create_dir_all(&dir).expect("temp data home");
    dir
}

#[test]
fn persistent_cookies_survive_browser_restart() {
    let data_home = temp_data_home();
    let profile = Profile::parse("work").expect("profile");
    let url = Url::parse("https://example.test/app").expect("url");
    {
        let browser =
            Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("browser");
        let tab = browser.handle().create_tab().expect("tab");
        tab.set_document_url(url.as_str()).expect("document url");
        tab.set_document_cookie("sid=1; Max-Age=3600; Path=/")
            .expect("set cookie");
        assert_eq!(tab.document_cookie().expect("cookie"), "sid=1");
    }
    let cookie_path = data_home
        .join("tinybrowser")
        .join("profiles")
        .join("work")
        .join("cookies");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cookie_path)
            .expect("cookie file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "cookie file mode {mode:#o}");
    }
    let browser = Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("browser");
    let tab = browser.handle().create_tab().expect("tab");
    tab.set_document_url(url.as_str()).expect("document url");
    assert_eq!(tab.document_cookie().expect("reloaded"), "sid=1");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn response_cookies_survive_browser_restart() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).expect("request");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=network; Max-Age=3600; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("response");
    });

    let data_home = temp_data_home();
    let profile = Profile::parse("network-cookie").expect("profile");
    let url = format!("http://{addr}/");
    {
        let browser =
            Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("browser");
        let tab = browser.handle().create_tab().expect("tab");
        tab.goto(&url).expect("goto");
        tab.run_until_load().expect("load");
        assert_eq!(tab.document_cookie().expect("live cookie"), "sid=network");
    }
    server.join().expect("server");

    let browser = Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("restart");
    let tab = browser.handle().create_tab().expect("tab");
    tab.set_document_url(&url).expect("document url");
    assert_eq!(tab.document_cookie().expect("stored cookie"), "sid=network");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn session_cookies_are_not_written_to_disk() {
    let data_home = temp_data_home();
    let profile = Profile::default();
    let url = Url::parse("https://example.test/").expect("url");
    {
        let browser =
            Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("browser");
        let tab = browser.handle().create_tab().expect("tab");
        tab.set_document_url(url.as_str()).expect("document url");
        tab.set_document_cookie("tmp=1").expect("session cookie");
        assert_eq!(tab.document_cookie().expect("cookie"), "tmp=1");
    }
    let browser = Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("browser");
    let tab = browser.handle().create_tab().expect("tab");
    tab.set_document_url(url.as_str()).expect("document url");
    assert_eq!(tab.document_cookie().expect("empty"), "");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn pages_share_the_profile_cookie_jar() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let handle = browser.handle();
    let first = handle.create_tab().expect("first");
    let second = handle.create_tab().expect("second");
    first
        .set_document_url("https://example.test/")
        .expect("url");
    second
        .set_document_url("https://example.test/")
        .expect("url");
    first
        .set_document_cookie("shared=1; Max-Age=60; Path=/")
        .expect("cookie");
    assert_eq!(second.document_cookie().expect("shared"), "shared=1");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn a_profile_has_exactly_one_writer() {
    let data_home = temp_data_home();
    let profile = Profile::default();
    let first =
        Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("first browser");

    let error = Browser::open_in_with(&data_home, &profile, Renderers::Local)
        .err()
        .expect("second writer must fail");

    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    drop(first);
    Browser::open_in_with(&data_home, &profile, Renderers::Local).expect("lock released");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn corrupt_cookie_data_is_quarantined_and_profile_recovers() {
    let data_home = temp_data_home();
    let profile_dir = data_home
        .join("tinybrowser")
        .join("profiles")
        .join("default");
    std::fs::create_dir_all(&profile_dir).expect("profile directory");
    std::fs::write(profile_dir.join("cookies"), b"not a cookie store").expect("corrupt cookies");

    let browser = Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local)
        .expect("recovered browser");
    let quarantined = std::fs::read_dir(&profile_dir)
        .expect("profile entries")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("cookies.corrupt.")
        });

    assert!(quarantined, "corrupt cookies were not quarantined");
    drop(browser);
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn close_page_returns_while_a_fetch_is_blocked() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Instant;

    fn read_head(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut head = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).expect("request read");
            assert_ne!(read, 0, "peer closed before request head");
            head.extend_from_slice(&chunk[..read]);
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accepted = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let accepted_flag = Arc::clone(&accepted);
    let flag = Arc::clone(&release);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        read_head(&mut stream);
        accepted_flag.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !flag.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "never released");
            thread::sleep(Duration::from_millis(10));
        }
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    });

    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let handle = browser.handle();
    let tab = handle.create_tab().expect("tab");
    tab.goto(&format!("http://{addr}/")).expect("goto");
    let waiting = tab.clone();
    let pump = thread::spawn(move || waiting.run_until_load());
    let wait_deadline = Instant::now() + Duration::from_secs(2);
    while !accepted.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < wait_deadline,
            "navigation never reached server"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    handle.close_tab(tab.id()).expect("close");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "close waited for blocked fetch: {:?}",
        started.elapsed()
    );
    release.store(true, Ordering::SeqCst);
    let _ = pump.join();
    server.join().expect("server");
    let _ = std::fs::remove_dir_all(data_home);
}
