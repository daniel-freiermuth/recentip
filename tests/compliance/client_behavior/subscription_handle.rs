use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use recentip::{EventId, EventgroupId, InstanceId, OfferedService, Runtime, RuntimeConfig};
use tracing::Instrument;

type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

// Wire values for ServiceA
const SERVICE_A_ID: u16 = 0x5674;
const SERVICE_A_VERSION: (u8, u32) = (1, 0);

#[test_log::test]
fn test_subscribe_drop_unsubscribes_in_time() {
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
                .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
                .version(SERVICE_A_VERSION.0, SERVICE_A_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let event_handle = offering
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(EventgroupId::new(1).unwrap())
                .create()
                .await
                .unwrap();

            loop {
                event_handle.notify("test".as_bytes()).await.unwrap();
                let target = tokio::time::Instant::now() + Duration::from_secs(1);
                let mut remaining = target - tokio::time::Instant::now();
                while remaining.as_secs_f32() > 0.0 {
                    if let Ok(Some(event)) = tokio::time::timeout(remaining, offering.next()).await
                    {
                        match event {
                            recentip::ServiceEvent::Unsubscribe { .. } => {
                                received_unsub
                                    .lock()
                                    .unwrap()
                                    .replace(tokio::time::Instant::now());
                            }
                            _ => {}
                        }
                    }
                    remaining = target - tokio::time::Instant::now();
                }
            }
        }
    });

    let unsub_send_time_clone = unsub_send_time.clone();
    sim.client("client", async move {
        let runtime_config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(runtime_config).await.unwrap();

        let service = runtime
            .find(SERVICE_A_ID)
            .instance(InstanceId::Id(1))
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
    let delay_expectation = Duration::from_millis(60);

    assert!(
        unsub_delay < delay_expectation,
        "Unsubscribe delay too high: {:?} > {:?}",
        unsub_delay,
        delay_expectation
    );
}

#[test_log::test]
fn test_two_subscribers_one_drops() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        async move {
            let runtime: TurmoilRuntime =
                Runtime::with_socket_type(Default::default()).await.unwrap();

            // Offer instance 1
            let mut offering = runtime
                .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
                .version(SERVICE_A_VERSION.0, SERVICE_A_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let event_handle = offering
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(EventgroupId::new(1).unwrap())
                .create()
                .await
                .unwrap();

            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tracing::info!("Server: Sending event");
                event_handle.notify("test".as_bytes()).await.unwrap();
                if let Some(event) = offering.next().await {
                    match event {
                        recentip::ServiceEvent::Unsubscribe { .. } => {
                            tracing::info!("Server: Received unsubscribe");
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    sim.client("client", async move {
        let runtime_config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(runtime_config).await.unwrap();

        let service = runtime
            .find(SERVICE_A_ID)
            .instance(InstanceId::Id(1))
            .await
            .unwrap();

        let flow1 = async {
            let mut subscription = service
                .subscribe(EventgroupId::new(1).unwrap())
                .await
                .unwrap();

            let now = tokio::time::Instant::now();
            while now.elapsed() < Duration::from_secs(5) {
                let _event = subscription.next().await.unwrap();
                tracing::info!("Received event in flow 1");
            }

            drop(subscription);
        };
        let flow2 = async {
            let mut subscription = service
                .subscribe(EventgroupId::new(1).unwrap())
                .await
                .unwrap();

            let mut event_count = 0;
            let now = tokio::time::Instant::now();
            while now.elapsed() < Duration::from_secs(15) {
                let _event = subscription.next().await.unwrap();
                tracing::info!("Received event in flow 2");
                event_count += 1;
            }
            assert!(
                event_count > 14,
                "Did not receive enough events: {}",
                event_count
            );
        };

        let _t = tokio::join!(flow1, flow2);

        runtime.shutdown().await;

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_dangling_subscription_cannot_unsubscribe() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        async move {
            let runtime: TurmoilRuntime =
                Runtime::with_socket_type(Default::default()).await.unwrap();

            // Offer instance 1
            let mut offering = runtime
                .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
                .version(SERVICE_A_VERSION.0, SERVICE_A_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let event_handle = offering
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(EventgroupId::new(1).unwrap())
                .create()
                .await
                .unwrap();

            for _ in 0..10 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tracing::info!("Server: Sending event");
                event_handle.notify("test".as_bytes()).await.unwrap();
                if let Some(event) = offering.next().await {
                    match event {
                        recentip::ServiceEvent::Unsubscribe { .. } => {
                            tracing::info!("Server: Received unsubscribe");
                        }
                        _ => {}
                    }
                }
            }

            drop(offering);
            tokio::time::sleep(Duration::from_secs(4)).await;

            let mut offering2 = runtime
                .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
                .version(SERVICE_A_VERSION.0, SERVICE_A_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let event_handle2 = offering2
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(EventgroupId::new(1).unwrap())
                .create()
                .await
                .unwrap();

            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tracing::info!("Server: Sending event");
                event_handle2.notify("test".as_bytes()).await.unwrap();
                if let Some(event) = offering2.next().await {
                    match event {
                        recentip::ServiceEvent::Unsubscribe { .. } => {
                            tracing::info!("Server: Received unsubscribe");
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    sim.client("client", async move {
        let runtime_config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(runtime_config).await.unwrap();

        let service = runtime
            .find(SERVICE_A_ID).instance(InstanceId::Id(1))

            .await
            .unwrap();

        let mut subscription1 = service
            .subscribe(EventgroupId::new(1).unwrap())
            .await
            .unwrap();

        while let Some(_) = tokio::time::timeout(Duration::from_secs(5), subscription1.next()).await.expect("This should not time out. When the server stops sending events, we should get None as a result of the StopOffer.") {
            tracing::info!("Received event for sub1");
        }
        tracing::info!("Sub1 seems dead");

        // Wait for the server to re-offer the service before trying to subscribe again
        // The server has a 4-second delay between dropping the offering and re-offering
        tokio::time::sleep(Duration::from_secs(5)).await;

        let mut subscription2 = service
            .subscribe(EventgroupId::new(1).unwrap())
            .await
            .unwrap();

        let mut event_count = 0;
        let now = tokio::time::Instant::now();
        while now.elapsed() < Duration::from_secs(5) {
            let _event = subscription2.next().await.unwrap();
            tracing::info!("Received event in flow 1");
            event_count += 1;
        }
        drop(subscription1);
        while now.elapsed() < Duration::from_secs(10) {
            let _event = subscription2.next().await.unwrap();
            tracing::info!("Received event in flow 1");
            event_count += 1;
        }
        assert!(event_count > 8, "Did not receive enough events: {}", event_count);

        runtime.shutdown().await;

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_two_subscribers_get_events() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        async move {
            let runtime: TurmoilRuntime =
                Runtime::with_socket_type(Default::default()).await.unwrap();

            // Offer instance 1
            let mut offering = runtime
                .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
                .version(SERVICE_A_VERSION.0, SERVICE_A_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let event_handle = offering
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(EventgroupId::new(1).unwrap())
                .create()
                .await
                .unwrap();

            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tracing::info!("Server: Sending event");
                event_handle.notify("test".as_bytes()).await.unwrap();
                if let Some(event) = offering.next().await {
                    match event {
                        recentip::ServiceEvent::Unsubscribe { .. } => {
                            tracing::info!("Server: Received unsubscribe");
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    sim.client("client", async move {
        async fn subscribe_and_listen(service: OfferedService) {
            tracing::info!("Starting subscription flow");
            let mut subscription = service
                .subscribe(EventgroupId::new(1).unwrap())
                .await
                .unwrap();

            let now = tokio::time::Instant::now();
            let mut event_count = 0;
            while now.elapsed() < Duration::from_secs(15) {
                let _event = subscription.next().await.unwrap();
                tracing::info!("Received event in flow");
                event_count += 1;
            }
            assert!(
                event_count > 12,
                "Did not receive enough events: {}",
                event_count
            );
        }

        let runtime_config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(runtime_config).await.unwrap();

        let service = runtime
            .find(SERVICE_A_ID)
            .instance(InstanceId::Id(1))
            .await
            .unwrap();

        let flow1 = subscribe_and_listen(service.clone()).instrument(tracing::info_span!("flow1"));
        let flow2 = subscribe_and_listen(service.clone()).instrument(tracing::info_span!("flow2"));
        let _t = tokio::join!(flow1, flow2);

        runtime.shutdown().await;

        Ok(())
    });

    sim.run().unwrap();
}
