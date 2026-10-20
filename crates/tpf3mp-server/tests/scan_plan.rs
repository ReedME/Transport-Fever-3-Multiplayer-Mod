//! Ordinary room APIs agree the experimental scanner before changing seats.
#![allow(clippy::unwrap_used)]
mod common;
use common::{FAST, RunningServer, join, room};
use std::sync::Arc;
use tpf3mp_agent::ClientError;
use tpf3mp_proto::{RequestError, ScanPlan, Text};

#[tokio::test]
async fn create_join_and_reconnect_require_the_exact_plan_before_seat_mutation() {
    let server = RunningServer::start(|_| {}).await;
    let mut owner = server.client("owner").await;
    let guest = server.client("guest").await;
    let plan = ScanPlan::from_values(Some("on"), Some("5x10")).unwrap();
    let settings = tpf3mp_proto::RoomSettings {
        scan_plan: plan,
        ..FAST
    };
    let mut create = room("experimental", settings);
    create.password = Some(Text::new("secret").unwrap());
    let (invite, created) = owner.client.create_room(create).await.unwrap();
    assert_eq!(created.settings.scan_plan, plan);
    let mut request = join(&invite);
    // An unauthenticated caller learns nothing about the scanner.
    assert_eq!(
        guest.client.join_room(request.clone()).await,
        Err(ClientError::Refused(RequestError::BadInvite))
    );
    request.password = Some(Text::new("secret").unwrap());
    for other in [
        ScanPlan::DEFAULT,
        ScanPlan::from_values(Some("compare"), Some("5x10")).unwrap(),
        ScanPlan::from_values(Some("on"), Some("6x10")).unwrap(),
        ScanPlan::from_values(Some("on"), Some("5x11")).unwrap(),
        ScanPlan { parts: 0, ..plan },
    ] {
        request.scan_plan = other;
        let probe = server.client("probe").await;
        assert_eq!(
            probe.client.join_room(request.clone()).await,
            Err(ClientError::Refused(RequestError::ScanPlanMismatch))
        );
    }
    request.scan_plan = plan;
    let joined = guest.client.join_room(request.clone()).await.unwrap();
    assert_eq!(joined.settings.scan_plan, plan);
    assert_eq!(joined.members.len(), 2);
    owner.room_where(|room| room.members.len() == 2).await;
    let replacement = server
        .client_as(Arc::clone(&guest.identity), "replacement")
        .await;
    request.scan_plan = ScanPlan::DEFAULT;
    assert_eq!(
        replacement.client.join_room(request.clone()).await,
        Err(ClientError::Refused(RequestError::ScanPlanMismatch))
    );
    // A failed replacement must leave the original connection in its seat.
    guest.client.set_ready(true).await.unwrap();
    let unchanged = owner
        .room_where(|room| {
            room.members
                .iter()
                .any(|m| m.player == guest.client.player() && m.ready)
        })
        .await;
    assert_eq!(
        unchanged
            .members
            .iter()
            .find(|m| m.player == guest.client.player())
            .unwrap()
            .name
            .as_str(),
        "guest"
    );
    request.scan_plan = plan;
    let rejoined = replacement.client.join_room(request).await.unwrap();
    assert_eq!(rejoined.members.len(), 2);
    assert_eq!(rejoined.settings.scan_plan, plan);
    server.shut_down().await;
}

#[tokio::test]
async fn invalid_scan_plans_cannot_create_a_room() {
    let server = RunningServer::start(|_| {}).await;
    let owner = server.client("owner").await;
    for plan in [
        ScanPlan {
            parts: 0,
            ..ScanPlan::DEFAULT
        },
        ScanPlan {
            parts: 5,
            ..ScanPlan::DEFAULT
        },
    ] {
        let settings = tpf3mp_proto::RoomSettings {
            scan_plan: plan,
            ..FAST
        };
        assert!(
            owner
                .client
                .create_room(room("bad", settings))
                .await
                .is_err()
        );
    }
    assert_eq!(server.stats.rooms(), 0);
    server.shut_down().await;
}
