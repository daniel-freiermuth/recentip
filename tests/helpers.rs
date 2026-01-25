use recentip::ServiceOffering;

pub(crate) async fn wait_for_subscription(offering: &mut ServiceOffering) -> Result<(), ()> {
    loop {
        match offering.next().await {
            Some(e) => match e {
                recentip::ServiceEvent::Subscribe { .. } => break Ok(()),
                _ => continue,
            },
            None => break Err(()),
        }
    }
}

pub(crate) fn configure_tracing() {
    use std::sync::OnceLock;
    static TRACING_INIT: OnceLock<()> = OnceLock::new();
    TRACING_INIT.get_or_init(|| {
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::builder()
                        .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
                        .from_env_lossy(),
                )
                .with_test_writer()
                .with_timer(SimElapsedTime)
                .finish(),
        )
        .expect("Configure tracing");
    });
}

#[derive(Clone)]
struct SimElapsedTime;
impl tracing_subscriber::fmt::time::FormatTime for SimElapsedTime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        // Prints real time and sim elapsed time. Example: 2024-01-10T17:06:57.020452Z [76ms]
        tracing_subscriber::fmt::time()
            .format_time(w)
            .and_then(|()| write!(w, " [{:?}]", turmoil::sim_elapsed().unwrap_or_default()))
    }
}
