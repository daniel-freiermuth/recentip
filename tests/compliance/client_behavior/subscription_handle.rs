use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use someip_runtime::{EventId, EventgroupId, InstanceId, Runtime, RuntimeConfigBuilder, Service};

type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

struct ServiceA;
impl Service for ServiceA {
    const SERVICE_ID: u16 = 0x5674;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

#[test_log::test]
#[ignore = "documenting a known problem"]
fn test_subscribe_drop_unsubscribes() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let unsub_send_time = Arc::new(Mutex::new(None));
    let unsub_receive_time = Arc::new(Mutex::new(None));

    let unsub_receive_time_clone = unsub_receive_time.clone();
    sim.host("server", move || {
        let received_unsub = unsub_receive_time_clone.clone();
        async move {
            let runtime: TurmoilRuntime =
                Runtime::with_socket_type(Default::default()).await.unwrap();

            // Offer instance 1
            let mut offering = runtime
                .offer::<ServiceA>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                offering
                    .notify(
                        EventgroupId::new(1).unwrap(),
                        EventId::new(0x8001).unwrap(),
                        "test".as_bytes(),
                    )
                    .await
                    .unwrap();
                if let Some(event) = offering.next().await {
                    match event {
                        someip_runtime::ServiceEvent::Unsubscribe { .. } => {
                            received_unsub
                                .lock()
                                .unwrap()
                                .replace(tokio::time::Instant::now());
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    let unsub_send_time_clone = unsub_send_time.clone();
    sim.client("client", async move {
        let runtime_config = RuntimeConfigBuilder::default()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(runtime_config).await.unwrap();

        let service = runtime
            .find::<ServiceA>(InstanceId::new(1).unwrap())
            .available()
            .await
            .unwrap();

        let mut subscription = service
            .subscribe(EventgroupId::new(1).unwrap())
            .await
            .unwrap();

        let _event = subscription.next().await.unwrap();

        drop(subscription);
        unsub_send_time_clone
            .lock()
            .unwrap()
            .replace(tokio::time::Instant::now());

        tokio::time::sleep(Duration::from_secs(5)).await;

        runtime.shutdown().await;

        tokio::time::sleep(Duration::from_secs(5)).await;

        Ok(())
    });

    sim.run().unwrap();
    let unsub_delay =
        unsub_receive_time.lock().unwrap().unwrap() - unsub_send_time.lock().unwrap().unwrap();
    let delay_expectation = Duration::from_millis(50);

    assert!(
        unsub_delay < delay_expectation,
        "Unsubscribe delay too high: {:?} > {:?}", unsub_delay, delay_expectation
    );
}
