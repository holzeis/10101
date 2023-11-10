use anyhow::Result;
use native::api;
use tests_e2e::app::run_app;
use tests_e2e::logger::init_tracing;
use tests_e2e::setup;
use tokio::task::spawn_blocking;

#[tokio::test]
#[ignore = "need to be run with 'just e2e' command"]
async fn app_can_be_restored_from_a_backup() -> Result<()> {
    init_tracing();

    let test = setup::TestSetup::new_with_open_position().await;

    let seed_phrase = api::get_seed_phrase();

    let ln_balance = test
        .app
        .rx
        .wallet_info()
        .expect("to have wallet info")
        .balances
        .lightning;

    // kill the app
    test.app.stop();
    tracing::info!("Shutting down app!");

    let app = run_app(Some(seed_phrase.0)).await;

    assert_eq!(
        app.rx
            .wallet_info()
            .expect("to have wallet info")
            .balances
            .lightning,
        ln_balance
    );

    let positions = spawn_blocking(|| api::get_positions().expect("Failed to get positions"))
        .await
        .unwrap();
    assert_eq!(1, positions.len());

    Ok(())
}

#[tokio::test]
#[ignore = "need to be run with 'just e2e' command"]
async fn app_can_be_restored_from_a_full_backup() -> Result<()> {
    init_tracing();

    let test = setup::TestSetup::new_with_open_position().await;

    let seed_phrase = api::get_seed_phrase();

    let ln_balance = test
        .app
        .rx
        .wallet_info()
        .expect("to have wallet info")
        .balances
        .lightning;

    spawn_blocking(|| api::full_backup().expect("Failed to run full backup"))
        .await
        .unwrap();

    // kill the app
    test.app.stop();
    tracing::info!("Shutting down app!");

    let app = run_app(Some(seed_phrase.0)).await;

    assert_eq!(
        app.rx
            .wallet_info()
            .expect("to have wallet info")
            .balances
            .lightning,
        ln_balance
    );

    let positions = spawn_blocking(|| api::get_positions().expect("Failed to get positions"))
        .await
        .unwrap();
    assert_eq!(1, positions.len());

    Ok(())
}
