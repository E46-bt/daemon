use std::collections::BTreeSet;

use anyhow::Result;
use bluer::{
    adv::{Advertisement, AdvertisementHandle},
    gatt::local::{
        Application, ApplicationHandle, Characteristic, CharacteristicNotify,
        CharacteristicNotifyMethod, CharacteristicWrite, CharacteristicWriteMethod, Service,
    },
    Adapter, Session,
};
use carplay_protocol::{DspCommand, ServiceMessage};
use uuid::{uuid, Uuid};

use super::Hub;

// Custom 128-bit UUIDs for carplay-audio BLE service.
// These are arbitrary — just need to be unique and consistent with the mobile app.
const SERVICE_UUID: Uuid = uuid!("cafecafe-cafe-cafe-cafe-cafecafe0001");
// Write-without-response: mobile → Pi, newline-delimited JSON DspCommand
const CHAR_CMD_UUID: Uuid = uuid!("cafecafe-cafe-cafe-cafe-cafecafe0002");
// Notify: Pi → mobile, newline-delimited JSON ServiceMessage
const CHAR_STATS_UUID: Uuid = uuid!("cafecafe-cafe-cafe-cafe-cafecafe0003");

// Handles kept alive for the duration of the BLE session.
struct BleSession {
    _adv: AdvertisementHandle,
    _app: ApplicationHandle,
}

pub async fn serve(hub: Hub) -> Result<()> {
    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    eprintln!("[ble] adapter: {}", adapter.name());

    let _session = register(&adapter, hub).await?;

    // Keep the task alive — BLE stops if we drop the handles.
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

async fn register(adapter: &Adapter, hub: Hub) -> Result<BleSession> {
    let hub_write = hub.clone();
    let hub_notify = hub.clone();

    let cmd_char = Characteristic {
        uuid: CHAR_CMD_UUID,
        write: Some(CharacteristicWrite {
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(Box::new(move |value, _req| {
                let hub = hub_write.clone();
                Box::pin(async move {
                    if let Ok(text) = std::str::from_utf8(&value) {
                        if let Ok(cmd) = serde_json::from_str::<DspCommand>(text.trim()) {
                            hub.dispatch(cmd).await;
                        }
                    }
                    Ok(())
                })
            })),
            ..Default::default()
        }),
        ..Default::default()
    };

    let stats_char = Characteristic {
        uuid: CHAR_STATS_UUID,
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
                let hub = hub_notify.clone();
                Box::pin(async move {
                    let mut rx = hub.broadcast_tx.subscribe();

                    // Send current state immediately when a client subscribes
                    if let Ok(json) = serde_json::to_string(&ServiceMessage::State(
                        hub.state.read().await.clone(),
                    )) {
                        let _ = notifier.notify(json.into_bytes()).await;
                    }

                    loop {
                        match rx.recv().await {
                            Ok(msg) => {
                                if let Ok(json) = serde_json::to_string(&msg) {
                                    if notifier.notify(json.into_bytes()).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                })
            })),
            ..Default::default()
        }),
        ..Default::default()
    };

    let service = Service {
        uuid: SERVICE_UUID,
        primary: true,
        characteristics: vec![cmd_char, stats_char],
        ..Default::default()
    };

    let app = adapter
        .serve_gatt_application(Application {
            services: vec![service],
            ..Default::default()
        })
        .await?;

    let adv = adapter
        .advertise(Advertisement {
            service_uuids: BTreeSet::from([SERVICE_UUID]),
            local_name: Some("carplay-audio".to_string()),
            discoverable: Some(true),
            ..Default::default()
        })
        .await?;

    eprintln!("[ble] GATT server active, advertising as \"carplay-audio\"");
    Ok(BleSession { _adv: adv, _app: app })
}
