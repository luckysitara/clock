use clockit::{
    constants::derive_bonding_curve,
    engine::position_manager::{ExitReason, Position, PositionManager, Reservation},
};
use solana_sdk::pubkey::Pubkey;

const SOL: u64 = 1_000_000_000;

fn manager() -> PositionManager {
    // trailing 15%, hard stop 20%, TP 2.5x, dev dump 20%
    PositionManager::new(0.15, 0.20, 2.5, 0.20)
}

fn position(mint: Pubkey, dev: Option<Pubkey>, dev_initial_balance: u64) -> Position {
    Position {
        mint,
        bonding_curve: derive_bonding_curve(&mint).0,
        entry_price_sol: 1.0,
        highest_price_sol: 1.0,
        token_balance: 1_000,
        sol_invested_lamports: SOL / 10,
        dev_wallet: dev,
        dev_initial_balance,
        token_program: Pubkey::new_unique(),
        is_selling: false,
        leaders: Vec::new(),
    }
}

#[tokio::test]
async fn reservation_blocks_duplicate_buys_and_enforces_capital() {
    let pm = manager();
    let (a, b, c) = (Pubkey::new_unique(), Pubkey::new_unique(), Pubkey::new_unique());
    let max = SOL / 5; // room for two 0.1 SOL buys

    assert_eq!(pm.try_reserve(&a, SOL / 10, max).await, Reservation::Reserved);
    // Same mint while the first buy is still in flight
    assert_eq!(pm.try_reserve(&a, SOL / 10, max).await, Reservation::AlreadyActive);
    assert_eq!(pm.try_reserve(&b, SOL / 10, max).await, Reservation::Reserved);
    // Pending buys count toward the limit
    assert!(matches!(
        pm.try_reserve(&c, SOL / 10, max).await,
        Reservation::CapitalLimit { .. }
    ));

    // A failed buy frees its capital
    pm.release(&b).await;
    assert_eq!(pm.try_reserve(&c, SOL / 10, max).await, Reservation::Reserved);

    // Confirmed position keeps blocking the mint
    pm.add_position(position(a, None, 0)).await;
    assert_eq!(pm.try_reserve(&a, SOL / 10, SOL).await, Reservation::AlreadyActive);
}

#[tokio::test]
async fn price_updates_resolve_by_bonding_curve_pda() {
    let pm = manager();
    let mint = Pubkey::new_unique();
    pm.add_position(position(mint, None, 0)).await;
    let curve = derive_bonding_curve(&mint).0;

    assert!(pm.on_price_update(&Pubkey::new_unique(), 0.1).await.is_none());
    assert!(pm.on_price_update(&curve, 1.1).await.is_none());

    let (pos, reason) = pm.on_price_update(&curve, 0.79).await.expect("hard stop");
    assert_eq!(pos.mint, mint);
    assert_eq!(reason, ExitReason::HardStopLoss);
    // Already selling: no duplicate exit signal
    assert!(pm.on_price_update(&curve, 0.5).await.is_none());
}

#[tokio::test]
async fn dev_dump_uses_known_balance_threshold() {
    let pm = manager();
    let (mint, dev) = (Pubkey::new_unique(), Pubkey::new_unique());
    pm.add_position(position(mint, Some(dev), 1_000_000)).await;

    // Other wallets selling never trigger
    assert!(pm.check_dev_dump(&mint, &Pubkey::new_unique(), 900_000).await.is_none());
    // Dev selling below 20% of their entry bag does not trigger
    assert!(pm.check_dev_dump(&mint, &dev, 100_000).await.is_none());
    // Dev selling >= 20% triggers
    assert!(pm.check_dev_dump(&mint, &dev, 200_000).await.is_some());
}

#[tokio::test]
async fn dev_dump_with_unknown_balance_triggers_on_any_dev_sell() {
    let pm = manager();
    let (mint, dev) = (Pubkey::new_unique(), Pubkey::new_unique());
    pm.add_position(position(mint, Some(dev), 0)).await;
    assert!(pm.check_dev_dump(&mint, &dev, 1).await.is_some());
}

#[tokio::test]
async fn set_dev_initial_balance_updates_threshold() {
    let pm = manager();
    let (mint, dev) = (Pubkey::new_unique(), Pubkey::new_unique());
    pm.add_position(position(mint, Some(dev), 0)).await;
    pm.set_dev_initial_balance(&mint, 1_000_000).await;
    assert!(pm.check_dev_dump(&mint, &dev, 10).await.is_none());
    assert!(pm.check_dev_dump(&mint, &dev, 250_000).await.is_some());
}

#[tokio::test]
async fn positions_persist_to_state_file() {
    let dir = std::env::temp_dir().join(format!("clockit-test-{}", Pubkey::new_unique()));
    let path = dir.join("positions.json");
    let pm = manager().with_state_file(&path);
    let mint = Pubkey::new_unique();

    pm.add_position(position(mint, None, 42)).await;
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains(&mint.to_string()));
    assert!(saved.contains("\"dev_initial_balance\": 42"));

    pm.remove_position(&mint).await;
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains(&mint.to_string()));

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn leader_sell_triggers_exit_once() {
    let pm = manager();
    let (mint, leader, other) = (Pubkey::new_unique(), Pubkey::new_unique(), Pubkey::new_unique());
    let mut pos = position(mint, None, 0);
    pos.leaders = vec![leader];
    pm.add_position(pos).await;

    // Someone else selling is not our cue
    assert!(pm.check_leader_exit(&mint, &other).await.is_none());
    // The copied wallet selling is
    assert!(pm.check_leader_exit(&mint, &leader).await.is_some());
    // Already exiting: no second signal
    assert!(pm.check_leader_exit(&mint, &leader).await.is_none());
}

#[tokio::test]
async fn leaders_survive_restart_via_state_file() {
    let dir = std::env::temp_dir().join(format!("clockit-test-{}", Pubkey::new_unique()));
    let path = dir.join("positions.json");
    let mint = Pubkey::new_unique();
    let leader = Pubkey::new_unique();
    let pm = manager().with_state_file(&path);
    let mut pos = position(mint, None, 0);
    pos.leaders = vec![leader];
    pm.add_position(pos).await;

    let saved = clockit::engine::position_manager::load_saved_positions(&path);
    assert_eq!(saved[&mint].leaders, vec![leader.to_string()]);
    let _ = std::fs::remove_dir_all(dir);
}
