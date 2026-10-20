use clockit::{
    constants::{
        BUY_DISCRIMINATOR_BYTES, BUY_DISCRIMINATOR_U64, CREATE_DISCRIMINATOR_BYTES,
        CREATE_DISCRIMINATOR_U64, SELL_DISCRIMINATOR_BYTES, SELL_DISCRIMINATOR_U64,
    },
    decoders::{
        fast_is_discriminator, BondingCurveAccountPod, CreateInstructionView, PumpFunBuyPod,
        PumpFunSellPod,
    },
    engine::tip_engine::TipEngine,
};

#[test]
fn test_fast_discriminator_matching() {
    assert!(fast_is_discriminator(&BUY_DISCRIMINATOR_BYTES, BUY_DISCRIMINATOR_U64));
    assert!(fast_is_discriminator(&SELL_DISCRIMINATOR_BYTES, SELL_DISCRIMINATOR_U64));
    assert!(fast_is_discriminator(&CREATE_DISCRIMINATOR_BYTES, CREATE_DISCRIMINATOR_U64));

    let bogus = [0u8; 8];
    assert!(!fast_is_discriminator(&bogus, BUY_DISCRIMINATOR_U64));
}

#[test]
fn test_pumpfun_volume_accumulators_derivation() {
    use std::str::FromStr;
    use solana_sdk::pubkey::Pubkey;
    let pump_prog = Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P").unwrap();
    let expected_global_vol = Pubkey::from_str("Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y").unwrap();
    let expected_user_vol = Pubkey::from_str("FGFrX2q1iAjyAojjeyFDxXqdmvegjPpSWsrPmrJjeQ2f").unwrap();
    let user = Pubkey::from_str("BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s").unwrap();

    let (global_vol, _) = Pubkey::find_program_address(&[b"global_volume_accumulator"], &pump_prog);
    assert_eq!(global_vol, expected_global_vol);

    let (user_vol, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user.as_ref()], &pump_prog);
    assert_eq!(user_vol, expected_user_vol);
}

#[test]
fn test_zero_copy_buy_deserialization() {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&BUY_DISCRIMINATOR_BYTES);
    buffer.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // amount
    buffer.extend_from_slice(&500_000_000u64.to_le_bytes());   // max_sol_cost

    let buy = PumpFunBuyPod::read_from_raw(&buffer).expect("Should deserialize buy payload");
    assert_eq!(buy.amount, 1_000_000_000);
    assert_eq!(buy.max_sol_cost, 500_000_000);
}

#[test]
fn test_zero_copy_sell_deserialization() {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&SELL_DISCRIMINATOR_BYTES);
    buffer.extend_from_slice(&5_000_000u64.to_le_bytes());     // amount
    buffer.extend_from_slice(&250_000_000u64.to_le_bytes());   // min_sol_output

    let sell = PumpFunSellPod::read_from_raw(&buffer).expect("Should deserialize sell payload");
    assert_eq!(sell.amount, 5_000_000);
    assert_eq!(sell.min_sol_output, 250_000_000);
}

#[test]
fn test_zero_allocation_create_parsing() {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&CREATE_DISCRIMINATOR_BYTES);

    let name = "Clockit Token";
    buffer.extend_from_slice(&(name.len() as u32).to_le_bytes());
    buffer.extend_from_slice(name.as_bytes());

    let symbol = "CLOCK";
    buffer.extend_from_slice(&(symbol.len() as u32).to_le_bytes());
    buffer.extend_from_slice(symbol.as_bytes());

    let uri = "https://clockit.io/meta.json";
    buffer.extend_from_slice(&(uri.len() as u32).to_le_bytes());
    buffer.extend_from_slice(uri.as_bytes());

    let creator = [7u8; 32];
    buffer.extend_from_slice(&creator);

    let view = CreateInstructionView::parse(&buffer).expect("Should parse create instruction");
    assert_eq!(view.name, name);
    assert_eq!(view.symbol, symbol);
    assert_eq!(view.uri, uri);
    assert_eq!(view.creator.unwrap().to_bytes(), creator);
}

#[test]
fn test_bonding_curve_math() {
    let curve = BondingCurveAccountPod {
        virtual_token_reserves: 1_073_000_000_000_000,
        virtual_sol_reserves: 30_000_000_000, // 30 SOL
        real_token_reserves: 793_100_000_000_000,
        real_sol_reserves: 0,
        token_total_supply: 1_000_000_000_000_000,
        complete: false,
        creator: solana_sdk::pubkey::Pubkey::default(),
    };

    // Buy with 1 SOL (1_000_000_000 lamports) and 5% slippage (500 bps)
    let buy_res = curve.calculate_buy_output(1_000_000_000, 500).expect("Curve buy math failed");
    assert!(buy_res.tokens_out > 0);
    assert!(buy_res.max_sol_cost > 1_000_000_000);
    assert_eq!(buy_res.max_sol_cost, 1_050_000_000); // 1 SOL + 5% = 1.05 SOL

    // Price should be greater than 0
    assert!(buy_res.effective_price_sol > 0.0);
}

#[test]
fn test_dynamic_tip_engine() {
    let engine = TipEngine::new(100_000, 50_000_000, 0.65);

    // Negative EV (profit <= min tip) should return None
    assert_eq!(engine.calculate_optimal_tip(50_000), None);

    // Normal profit: 1,000,000 lamports profit * 0.65 = 650,000 lamports tip
    assert_eq!(engine.calculate_optimal_tip(1_000_000), Some(650_000));

    // Massive profit: capped at max tip
    assert_eq!(engine.calculate_optimal_tip(200_000_000), Some(50_000_000));
}

#[test]
fn test_slippage_and_apply_buy() {
    let mut curve = BondingCurveAccountPod {
        virtual_token_reserves: 1_073_000_000_000_000,
        virtual_sol_reserves: 30_000_000_000, // 30 SOL
        real_token_reserves: 793_100_000_000_000,
        real_sol_reserves: 0,
        token_total_supply: 1_000_000_000_000_000,
        complete: false,
        creator: solana_sdk::pubkey::Pubkey::default(),
    };

    // Dev buys 50_000_000_000_000 tokens in create tx
    let dev_cost = curve.apply_buy(50_000_000_000_000).expect("Dev buy failed");
    assert!(dev_cost > 0);
    assert!(curve.virtual_sol_reserves > 30_000_000_000);
    assert_eq!(curve.virtual_token_reserves, 1_073_000_000_000_000 - 50_000_000_000_000);

    // Calculate our snipe buy with 0.20 SOL and 25% slippage
    let snipe_res = curve.calculate_buy_output(200_000_000, 2500).expect("Snipe buy failed");
    assert!(snipe_res.tokens_out > 0);
    // max_sol_cost has 5% buffer over 0.20 SOL = 0.21 SOL
    assert_eq!(snipe_res.max_sol_cost, 210_000_000);
}

#[test]
fn test_arbitrage_optimal_math() {
    use clockit::decoders::arb_math::{calculate_optimal_arb_directional, ArbDirection, ConstantProductPool};

    // Pool A: 100 SOL, 10,000 tokens (spot: 0.010 SOL)
    let pool_a = ConstantProductPool::new(100_000_000_000, 10_000_000_000, 25);
    // Pool B: 120 SOL, 10,000 tokens (spot: 0.012 SOL)
    let pool_b = ConstantProductPool::new(120_000_000_000, 10_000_000_000, 25);

    let opp = calculate_optimal_arb_directional(&pool_a, &pool_b, ArbDirection::PoolAToPoolB)
        .expect("Should find arbitrage opportunity");

    assert!(opp.optimal_sol_in_lamports > 0);
    assert!(opp.gross_profit_lamports > 0);
    assert!(opp.expected_sol_out_lamports > opp.optimal_sol_in_lamports);
    // Gross profit should be approximately ~4.6 SOL range based on mathematical derivation
    assert!(opp.gross_profit_lamports > 300_000_000);
}

#[test]
fn test_arbitrage_no_opportunity_when_spread_too_small() {
    use clockit::decoders::arb_math::{calculate_optimal_arb_directional, ArbDirection, ConstantProductPool};

    // Pool A and Pool B have virtually identical spot price
    let pool_a = ConstantProductPool::new(100_000_000_000, 10_000_000_000, 25);
    let pool_b = ConstantProductPool::new(100_100_000_000, 10_000_000_000, 25);

    // Combined fees (0.25% + 0.25% = 0.50%) exceed 0.1% spread -> No profit
    let opp = calculate_optimal_arb_directional(&pool_a, &pool_b, ArbDirection::PoolAToPoolB);
    assert_eq!(opp, None);
}

#[test]
fn test_arbitrage_find_best_direction() {
    use clockit::decoders::arb_math::{find_best_arb, ArbDirection, ConstantProductPool};

    // Pool A is more expensive than Pool B
    let pool_a = ConstantProductPool::new(130_000_000_000, 10_000_000_000, 25);
    let pool_b = ConstantProductPool::new(100_000_000_000, 10_000_000_000, 25);

    let best = find_best_arb(&pool_a, &pool_b).expect("Should find arb");
    // Should automatically select PoolB -> PoolA
    assert_eq!(best.direction, ArbDirection::PoolBToPoolA);
    assert!(best.gross_profit_lamports > 0);
}

#[test]
fn test_arbitrage_bundle_transaction_size() {
    use solana_sdk::{
        signature::Keypair,
        signer::Signer,
        hash::Hash,
    };
    use clockit::{
        decoders::arb_math::{ArbOpportunity, ArbDirection},
        engine::instruction_builder::InstructionBuilder,
        engine::pool_registry::build_atomic_arb_bundle_for_mint,
        engine::jito::JitoClient,
    };

    let keypair = Keypair::new();
    let mint = Keypair::new().pubkey();

    let opp = ArbOpportunity {
        direction: ArbDirection::PoolAToPoolB,
        optimal_sol_in_lamports: 450_000_000,
        expected_tokens_intermediate: 10_000_000_000,
        expected_sol_out_lamports: 480_000_000,
        gross_profit_lamports: 30_000_000,
        roi_bps: 666,
    };

    let instructions = vec![
        InstructionBuilder::set_compute_unit_limit(350_000),
        InstructionBuilder::set_compute_unit_price(200_000),
    ];
    let arb_ixs = build_atomic_arb_bundle_for_mint(&keypair.pubkey(), &mint, &opp);

    // In Jito, multi-DEX atomic arbitrage is dispatched as a 2-tx bundle [tx1 (buy), tx2 (sell + tip)]
    use solana_sdk::{message::Message, transaction::Transaction};
    let jito_client = JitoClient::new("https://mainnet.block-engine.jito.wtf/api/v1/bundles".to_string());
    let tx1 = Transaction::new(
        &[&keypair],
        Message::new(&[instructions[0].clone(), instructions[1].clone(), arb_ixs[0].clone()], Some(&keypair.pubkey())),
        Hash::default(),
    );
    let tx2 = jito_client.build_tip_bundle(&keypair, vec![arb_ixs[1].clone()], 100_000, Hash::default());

    let ser1 = bincode::serialize(&tx1).expect("Must serialize tx1");
    let ser2 = bincode::serialize(&tx2).expect("Must serialize tx2");

    println!("Jito Arb Leg 1 (Buy) size: {} bytes", ser1.len());
    println!("Jito Arb Leg 2 (Sell + Tip) size: {} bytes", ser2.len());

    assert!(ser1.len() <= 1232, "Leg 1 exceeds Solana MTU limit: {} bytes", ser1.len());
    assert!(ser2.len() <= 1232, "Leg 2 exceeds Solana MTU limit: {} bytes", ser2.len());
}

#[test]
fn test_triangular_cycle_arbitrage() {
    use clockit::engine::triangular::TriangularCycle;
    use clockit::decoders::arb_math::ConstantProductPool;
    use solana_sdk::pubkey::Pubkey;

    // Triangle: SOL -> USDC (Pool 1) -> BONK (Pool 2) -> SOL (Pool 3)
    // Pool 1: 5,000 SOL : 750,000 USDC (Spot: 150 USDC per SOL)
    let p1 = ConstantProductPool::new(5_000_000_000_000, 750_000_000_000, 30);
    // Pool 2: 750,000 USDC : 48,000,000,000 BONK (Spot: 64,000 BONK per USDC)
    let p2 = ConstantProductPool::new(750_000_000_000, 48_000_000_000_000_000, 25);
    // Pool 3: 7,000 SOL : 48,000,000,000 BONK (Spot: 6,857,142 BONK per SOL -> Misaligned in favor of cycle!)
    let p3 = ConstantProductPool::new(7_000_000_000_000, 48_000_000_000_000_000, 20);

    let cycle = TriangularCycle::new(
        "SOL->USDC->BONK->SOL",
        "USDC",
        "BONK",
        p1,
        p2,
        p3,
        Pubkey::default(),
        Pubkey::default(),
        Pubkey::default(),
    );

    let opp = cycle.evaluate_amount(450_000_000, 1_000_000); // 0.45 SOL bullet
    assert!(opp.is_some(), "Should detect profitable triangular opportunity");
    let opp = opp.unwrap();
    assert!(opp.expected_sol_out_lamports > opp.optimal_sol_in_lamports);
    assert!(opp.gross_profit_lamports > 0);
}

#[test]
fn test_hybrid_registry_scan() {
    use clockit::engine::hybrid_registry::HybridRegistry;
    use clockit::engine::arb_engine::ArbitrageEngine;

    let registry = HybridRegistry::new_with_defaults();
    assert_eq!(registry.markets().len(), 4);
    assert_eq!(registry.cycles().len(), 2);

    let arb_engine = ArbitrageEngine::new(1_000_000, 450_000_000);
    let _spatial_opps = registry.scan_spatial_opportunities(&arb_engine);
    let _tri_opps = registry.scan_triangular_opportunities(450_000_000, 1_000_000);
}

#[test]
fn test_flash_loan_instructions() {
    use clockit::engine::flash_loan::{FlashLoanAccounts, FlashLoanManager};
    use solana_sdk::pubkey::Pubkey;

    let accounts = FlashLoanAccounts {
        program_id: Pubkey::new_unique(),
        reserve: Pubkey::new_unique(),
        reserve_vault: Pubkey::new_unique(),
        market: Pubkey::new_unique(),
        market_authority: Pubkey::new_unique(),
    };

    let user_wsol = Pubkey::new_unique();
    let borrow_ix = FlashLoanManager::build_flash_borrow(&accounts, &user_wsol, 50_000_000_000);
    assert_eq!(borrow_ix.data[0], 14u8);
    assert_eq!(borrow_ix.accounts.len(), 7);

    let repay_ix = FlashLoanManager::build_flash_repay(&accounts, &user_wsol, 50_000_000_000, 0);
    assert_eq!(repay_ix.data[0], 15u8);
    assert_eq!(repay_ix.accounts.len(), 7);

    let fee = FlashLoanManager::calculate_fee(50_000_000_000);
    assert_eq!(fee, 5_000_000); // 0.005 SOL fee for 50 SOL borrow
}

#[test]
fn test_migration_sniper_detection() {
    use clockit::engine::migration_sniper::MigrationSniper;
    use clockit::decoders::BondingCurveAccountPod;
    use solana_sdk::pubkey::Pubkey;

    let mut sniper = MigrationSniper::new(100_000_000);
    let mint = Pubkey::new_unique();

    let non_graduated = BondingCurveAccountPod {
        virtual_token_reserves: 1_000_000_000_000_000,
        virtual_sol_reserves: 30_000_000_000,
        real_token_reserves: 793_100_000_000_000,
        real_sol_reserves: 20_000_000_000,
        token_total_supply: 1_000_000_000_000_000,
        complete: false,
        creator: Pubkey::default(),
    };
    assert!(!sniper.is_graduating(&mint, &non_graduated));

    let graduated = BondingCurveAccountPod {
        virtual_token_reserves: 1_000_000_000_000_000,
        virtual_sol_reserves: 30_000_000_000,
        real_token_reserves: 0,
        real_sol_reserves: 85_000_000_000,
        token_total_supply: 1_000_000_000_000_000,
        complete: true,
        creator: Pubkey::default(),
    };
    assert!(sniper.is_graduating(&mint, &graduated));
    // Should dedup
    assert!(!sniper.is_graduating(&mint, &graduated));
}

#[test]
fn test_lst_depeg_evaluation() {
    use clockit::engine::lst_depeg::LstDepegEngine;
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    let engine = LstDepegEngine::new(40); // 40 bps
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RKmjeoKuwWYvgwpq26bqnuJaS52cvEZ28").unwrap();

    // Normal fair price: no depeg
    let opp_normal = engine.evaluate_price(&jitosol_mint, 1_127_000_000, 10_000_000_000);
    assert!(opp_normal.is_none());

    // Discounted price (1.115 SOL vs fair 1.127 SOL = ~106 bps discount)
    let opp_depeg = engine.evaluate_price(&jitosol_mint, 1_115_000_000, 10_000_000_000);
    assert!(opp_depeg.is_some());
    let opp = opp_depeg.unwrap();
    assert_eq!(opp.symbol, "JitoSOL");
    assert!(opp.discount_bps >= 40);
    assert!(opp.expected_profit_lamports > 0);
}

#[test]
fn test_backrun_dislocation_evaluation() {
    use clockit::engine::backrunner::BackrunEngine;
    use solana_sdk::pubkey::Pubkey;

    let engine = BackrunEngine::new(5_000_000_000, 100); // 5 SOL min, 1% dislocation
    let mint = Pubkey::new_unique();
    let swapper = Pubkey::new_unique();

    // Small retail swap: ignore
    let signal_small = engine.evaluate_swap(&mint, &swapper, 1_000_000_000, 100_000_000_000);
    assert!(signal_small.is_none());

    // Massive retail swap (8 SOL into 50 SOL pool = 16% dislocation)
    let signal_big = engine.evaluate_swap(&mint, &swapper, 8_000_000_000, 50_000_000_000);
    assert!(signal_big.is_some());
    let sig = signal_big.unwrap();
    assert_eq!(sig.dislocation_bps, 1600);
    assert!(sig.recommended_size_lamports >= 100_000_000);
}



#[test]
fn test_tip_caps_scale_with_trade_size() {
    use clockit::engine::tip_engine::JITO_MIN_TIP_LAMPORTS;
    // min 0.001 SOL, max 0.005 SOL, caps 1% normal / 3% panic
    let tips = TipEngine::new(1_000_000, 5_000_000, 0.65).with_trade_caps(0.01, 0.03);

    // 0.035 SOL trade: 1% = 350_000 lamports caps the 1_000_000 default
    assert_eq!(tips.trade_tip(35_000_000), 350_000);
    // Panic: 3% = 1_050_000 caps the 5_000_000 max
    assert_eq!(tips.panic_tip(35_000_000), 1_050_000);
    // Large trade: configured tips apply unchanged
    assert_eq!(tips.trade_tip(10_000_000_000), 1_000_000);
    assert_eq!(tips.panic_tip(10_000_000_000), 5_000_000);
    // Never below Jito's minimum
    assert_eq!(tips.trade_tip(0), JITO_MIN_TIP_LAMPORTS);
}
