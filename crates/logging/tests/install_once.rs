#[test]
fn a_second_install_is_an_error() {
    logging::install(logging::Level::Info, None, std::time::SystemTime::now)
        .expect("first install");
    assert!(matches!(
        logging::install(logging::Level::Info, None, std::time::SystemTime::now),
        Err(logging::InstallError::AlreadyInstalled)
    ));
}
