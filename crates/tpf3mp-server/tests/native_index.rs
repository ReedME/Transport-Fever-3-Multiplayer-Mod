//! A server's native mods (docs/NATIVE_MODS.md, "A server's own index"): a
//! server whose operator named no registry offers none, and says so.

#![allow(clippy::unwrap_used)]

mod common;

use common::RunningServer;

#[tokio::test]
async fn a_server_without_a_registry_offers_no_native_mods() {
    let server = RunningServer::start(|_| {}).await;
    let player = server.client("player").await;
    assert_eq!(player.client.native_index(false).await.unwrap(), None);
    assert_eq!(player.client.native_index(true).await.unwrap(), None);
}
