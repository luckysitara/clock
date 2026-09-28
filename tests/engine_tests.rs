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
    assert_eq!(view.creator.to_bytes(), creator);
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
