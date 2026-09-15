# Server-stack probe

Standalone crate for the axum-versus-hyper server-stack probe. Two binaries
serve the same HTTP discovery endpoint and a WebSocket echo endpoint:
`axum_server` uses `axum::extract::ws`, and `hyper_server` uses
`hyper::upgrade::on` plus `tokio-tungstenite`.

    cargo build --release --manifest-path tools/stackprobe/Cargo.toml
    stat -c '%n %s' tools/stackprobe/target/release/axum_server tools/stackprobe/target/release/hyper_server

The crate is not a workspace member and is not built by `cargo test
--workspace`.
