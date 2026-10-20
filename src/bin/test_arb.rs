use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::time::Instant;

use clockit::{
    constants,
    decoders::arb_math::{calculate_optimal_arb_directional, ArbDirection, ConstantProductPool},
    engine::{
        arb_engine::ArbitrageEngine,
        instruction_builder::RaydiumAmmAccounts,
        tip_engine::TipEngine,
    },
};


fn main() -> Result<()> {
    println!("==========================================================================");
    println!("       CLOCKIT: CROSS-DEX ATOMIC ARBITRAGE SIMULATOR & BENCHMARK");
    println!("==========================================================================");

    let tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);
    let arb_engine = ArbitrageEngine::new(10_000_000, 10_000_000_000); // 0.01 SOL min profit, 10 SOL max trade

    // -----------------------------------------------------------------------------------
    // Scenario 1: Pump.fun late curve vs newly migrated Raydium AMM pool
    // -----------------------------------------------------------------------------------
    println!("\n[Scenario 1] Pump.fun Curve vs. Early Raydium Pool");
    println!("--------------------------------------------------------------------------");
    // Pool A (Pump.fun curve): 85 SOL and 206.9M tokens (Spot: ~0.0000004108 SOL/token)
    let pumpfun_pool = ConstantProductPool::new(85_000_000_000, 206_900_000_000_000, 100); // 1.0% fee
    // Pool B (Raydium AMM): 102 SOL and 206.9M tokens (Spot: ~0.0000004929 SOL/token)
    let raydium_pool = ConstantProductPool::new(102_000_000_000, 206_900_000_000_000, 25); // 0.25% fee

    println!("Pool A (Pump.fun): Spot Price = {:.10} SOL | SOL Reserves: {:.2} SOL", pumpfun_pool.spot_price_sol(), pumpfun_pool.sol_reserves as f64 / 1e9);
    println!("Pool B (Raydium):  Spot Price = {:.10} SOL | SOL Reserves: {:.2} SOL", raydium_pool.spot_price_sol(), raydium_pool.sol_reserves as f64 / 1e9);

    if let Some(opp) = arb_engine.evaluate_pair(&pumpfun_pool, &raydium_pool) {
        let optimal_sol = opp.optimal_sol_in_lamports as f64 / 1e9;
        let expected_sol = opp.expected_sol_out_lamports as f64 / 1e9;
        let gross_profit = opp.gross_profit_lamports as f64 / 1e9;
        let optimal_tip = tip_engine
            .calculate_optimal_tip(opp.gross_profit_lamports)
            .unwrap_or_else(|| tip_engine.default_tip()) as f64
            / 1e9;
        let net_profit = gross_profit - optimal_tip;

        println!("⚡ Arbitrage Found!");
        println!("   Direction:              {:?}", opp.direction);
        println!("   Optimal Input Δx*:      {:.4} SOL ({} lamports)", optimal_sol, opp.optimal_sol_in_lamports);
        println!("   Tokens Acquired:        {}", opp.expected_tokens_intermediate);
        println!("   Expected SOL Returned:  {:.4} SOL ({} lamports)", expected_sol, opp.expected_sol_out_lamports);
        println!("   Gross Profit:           {:+.4} SOL (ROI: {:.2}%)", gross_profit, opp.roi_bps as f64 / 100.0);
        println!("   Dynamic Jito Tip (65%): {:.5} SOL", optimal_tip);
        println!("   NET PROFIT (After Tip): {:+.4} SOL", net_profit);
    } else {
        println!("❌ No profitable opportunity found.");
    }

    // -----------------------------------------------------------------------------------
    // Scenario 2: Two AMM pools with reverse direction (Pool B -> Pool A)
    // -----------------------------------------------------------------------------------
    println!("\n[Scenario 2] Cross-AMM Pair with Reverse Discrepancy (Pool B cheaper)");
    println!("--------------------------------------------------------------------------");
    let pool_1 = ConstantProductPool::new(500_000_000_000, 10_000_000_000_000, 25); // Raydium
    let pool_2 = ConstantProductPool::new(480_000_000_000, 10_000_000_000_000, 30); // Orca

    if let Some(opp) = arb_engine.evaluate_pair(&pool_1, &pool_2) {
        println!("⚡ Arbitrage Found!");
        println!("   Direction:              {:?}", opp.direction);
        println!("   Optimal Input:          {:.4} SOL", opp.optimal_sol_in_lamports as f64 / 1e9);
        println!("   Gross Profit:           {:+.4} SOL (ROI: {:.2}%)", opp.gross_profit_lamports as f64 / 1e9, opp.roi_bps as f64 / 100.0);
    }

    // -----------------------------------------------------------------------------------
    // Scenario 3: Verify Atomic Instruction Assembly & Profit Enforcement
    // -----------------------------------------------------------------------------------
    println!("\n[Scenario 3] Atomic Instruction Assembly Verification");
    println!("--------------------------------------------------------------------------");
    let user = Pubkey::new_unique();
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let token_program = constants::spl_token_program_id();


    let fake_raydium_accounts = RaydiumAmmAccounts {
        amm_id: Pubkey::new_unique(),
        amm_authority: Pubkey::new_unique(),
        amm_open_orders: Pubkey::new_unique(),
        amm_target_orders: Pubkey::new_unique(),
        pool_coin_token_account: Pubkey::new_unique(),
        pool_pc_token_account: Pubkey::new_unique(),
        serum_program_id: Pubkey::new_unique(),
        serum_market: Pubkey::new_unique(),
        serum_bids: Pubkey::new_unique(),
        serum_asks: Pubkey::new_unique(),
        serum_event_queue: Pubkey::new_unique(),
        serum_coin_vault_account: Pubkey::new_unique(),
        serum_pc_vault_account: Pubkey::new_unique(),
        serum_vault_signer: Pubkey::new_unique(),
        user_source_token_account: Pubkey::new_unique(),
        user_destination_token_account: Pubkey::new_unique(),
        user_source_owner: user,
    };

    if let Some(opp) = arb_engine.evaluate_pair(&pumpfun_pool, &raydium_pool) {
        let ixs = arb_engine.build_pumpfun_raydium_arb(
            &user,
            &mint,
            &creator,
            &token_program,
            &fake_raydium_accounts,
            &opp,
        );
        println!("✅ Built {} atomic instructions for Solana transaction bundle:", ixs.len());
        println!("   1. Idempotent ATA Create Instruction (Program: {})", ixs[0].program_id);
        println!("   2. Leg 1 Swap (Program: {})", ixs[1].program_id);
        println!("   3. Leg 2 Swap with Strict Profit Floor (Program: {})", ixs[2].program_id);
    }

    // -----------------------------------------------------------------------------------
    // Scenario 4: Nanosecond Decision Latency Benchmark (100,000 iterations)
    // -----------------------------------------------------------------------------------
    println!("\n[Scenario 4] Execution Latency Benchmark (100,000 iterations)");
    println!("--------------------------------------------------------------------------");
    let iterations = 100_000;
    let bench_start = Instant::now();
    for _ in 0..iterations {
        let _ = calculate_optimal_arb_directional(&pumpfun_pool, &raydium_pool, ArbDirection::PoolAToPoolB);
    }
    let total_elapsed = bench_start.elapsed();
    let avg_nanos = total_elapsed.as_nanos() / iterations;

    println!("   Total Benchmark Duration: {:?}", total_elapsed);
    println!("   Average Calculation Time: {} nanoseconds (~{:.3} µs) per pair check!", avg_nanos, avg_nanos as f64 / 1000.0);
    println!("==========================================================================\n");

    Ok(())
}
