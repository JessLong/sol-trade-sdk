use anyhow::anyhow;
use borsh::BorshDeserialize;
use spl_token::instruction::close_account;
use tokio::sync::RwLock;
use std::{collections::HashMap, sync::Arc};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer, system_instruction, transaction::Transaction
};
use spl_associated_token_account::get_associated_token_address;
use pumpfun_program::accounts::BondingCurveAccount as PumpfunBondingCurveAccount;
use crate::{accounts::{self, BondingCurveAccount}, common::{pumpfun::logs_data::TradeInfo, PriorityFee, SolanaRpcClient}, constants::{self, pumpfun::{self, global_constants::{CREATOR_FEE, FEE_BASIS_POINTS}, trade::DEFAULT_SLIPPAGE}}};

lazy_static::lazy_static! {
    static ref ACCOUNT_CACHE: RwLock<HashMap<Pubkey, Arc<accounts::GlobalAccount>>> = RwLock::new(HashMap::new());
}

pub async fn transfer_sol(rpc: &SolanaRpcClient, payer: &Keypair, receive_wallet: &Pubkey, amount: u64) -> Result<(), anyhow::Error> {
    if amount == 0 {
        return Err(anyhow!("transfer_sol: Amount cannot be zero"));
    }

    let balance = get_sol_balance(rpc, &payer.pubkey()).await?;
    if balance < amount {
        return Err(anyhow!("Insufficient balance"));
    }

    let transfer_instruction = system_instruction::transfer(
        &payer.pubkey(),
        receive_wallet,
        amount,
    );

    let recent_blockhash = rpc.get_latest_blockhash().await?;

    let transaction = Transaction::new_signed_with_payer(
        &[transfer_instruction],
        Some(&payer.pubkey()),
        &[payer],
        recent_blockhash,
    );  

    rpc.send_and_confirm_transaction(&transaction).await?;

    Ok(())
}

/// 关闭代币账户
///
/// 此函数用于关闭指定代币的关联代币账户，将账户中的代币余额转移给账户所有者。
///
/// # 参数
///
/// * `rpc` - Solana RPC客户端
/// * `payer` - 支付交易费用的账户
/// * `mint` - 代币的Mint地址
///
/// # 返回值
///
/// 返回一个Result，成功时返回()，失败时返回错误
pub async fn close_token_account(rpc: &SolanaRpcClient, payer: &Keypair, mint: &Pubkey) -> Result<(), anyhow::Error> {
    // 获取关联代币账户地址
    let ata = get_associated_token_address(&payer.pubkey(), mint);
    
    // 检查账户是否存在
    let account_exists = rpc.get_account(&ata).await.is_ok();
    if !account_exists {
        return Ok(()); // 如果账户不存在，直接返回成功
    }
    
    // 构建关闭账户指令
    let close_account_ix = close_account(
        &spl_token::ID,
        &ata,
        &payer.pubkey(),
        &payer.pubkey(),
        &[&payer.pubkey()],
    )?;

    // 构建交易
    let recent_blockhash = rpc.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[close_account_ix],
        Some(&payer.pubkey()),
        &[payer],
        recent_blockhash,
    );

    // 发送交易
    rpc.send_and_confirm_transaction(&transaction).await?;

    Ok(())
}

#[inline]
pub fn create_priority_fee_instructions(priority_fee: PriorityFee) -> Vec<Instruction> {
    let mut instructions = Vec::with_capacity(2);
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(priority_fee.unit_limit));
    instructions.push(ComputeBudgetInstruction::set_compute_unit_price(priority_fee.unit_price));
    
    instructions
}

// #[inline]
pub async fn get_token_balance(rpc: &SolanaRpcClient, payer: &Pubkey, mint: &Pubkey) -> Result<u64, anyhow::Error> {
    let ata = get_associated_token_address(payer, mint);
    // let account_data = rpc.get_account_data(&ata).await?;
    // let token_account = Account::unpack(&account_data.as_slice())?;

    // Ok(token_account.amount)

    // println!("get_token_balance ata: {}", ata);
    let balance = rpc.get_token_account_balance(&ata).await?;
    let balance_u64 = balance.amount.parse::<u64>()
        .map_err(|_| anyhow!("Failed to parse token balance"))?;
    Ok(balance_u64)
}

#[inline]
pub async fn get_token_balance_and_ata(rpc: &SolanaRpcClient, payer: &Keypair, mint: &Pubkey) -> Result<(u64, Pubkey), anyhow::Error> {
    let ata = get_associated_token_address(&payer.pubkey(), mint);
    // let account_data = rpc.get_account_data(&ata).await?;
    // let token_account = Account::unpack(&account_data)?;

    // Ok((token_account.amount, ata))

    let balance = rpc.get_token_account_balance(&ata).await?;
    let balance_u64 = balance.amount.parse::<u64>()
        .map_err(|_| anyhow!("Failed to parse token balance"))?;
    
    if balance_u64 == 0 {
        return Err(anyhow!("Balance is 0"));
    }

    Ok((balance_u64, ata))
}

#[inline]
pub async fn get_sol_balance(rpc: &SolanaRpcClient, account: &Pubkey) -> Result<u64, anyhow::Error> {
    let balance = rpc.get_balance(account).await?;
    Ok(balance)
}

#[inline]
pub fn get_global_pda() -> Pubkey {
    static GLOBAL_PDA: once_cell::sync::Lazy<Pubkey> = once_cell::sync::Lazy::new(|| {
        Pubkey::find_program_address(&[constants::pumpfun::seeds::GLOBAL_SEED], &constants::pumpfun::accounts::PUMPFUN).0
    });
    *GLOBAL_PDA
}

#[inline]
pub fn get_mint_authority_pda() -> Pubkey {
    static MINT_AUTHORITY_PDA: once_cell::sync::Lazy<Pubkey> = once_cell::sync::Lazy::new(|| {
        Pubkey::find_program_address(&[constants::pumpfun::seeds::MINT_AUTHORITY_SEED], &constants::pumpfun::accounts::PUMPFUN).0
    });
    *MINT_AUTHORITY_PDA
}

#[inline]
pub fn get_bonding_curve_pda(mint: &Pubkey) -> Option<Pubkey> {
    let seeds: &[&[u8]; 2] = &[constants::pumpfun::seeds::BONDING_CURVE_SEED, mint.as_ref()];
    let program_id: &Pubkey = &constants::pumpfun::accounts::PUMPFUN;
    let pda: Option<(Pubkey, u8)> = Pubkey::try_find_program_address(seeds, program_id);
    pda.map(|pubkey| pubkey.0)
}

#[inline]
pub fn get_creator_vault_pda(creator: &Pubkey) -> Option<Pubkey> {
    let seeds: &[&[u8]; 2] = &[constants::pumpfun::seeds::CREATOR_VAULT_SEED, creator.as_ref()];
    let program_id: &Pubkey = &constants::pumpfun::accounts::PUMPFUN;
    let pda: Option<(Pubkey, u8)> = Pubkey::try_find_program_address(seeds, program_id);
    pda.map(|pubkey| pubkey.0)
}

#[inline]
pub fn get_metadata_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            constants::pumpfun::seeds::METADATA_SEED,
            constants::pumpfun::accounts::MPL_TOKEN_METADATA.as_ref(),
            mint.as_ref(),
        ],
        &constants::pumpfun::accounts::MPL_TOKEN_METADATA
    ).0
}

#[inline]
pub async fn get_global_account(/*rpc: &SolanaRpcClient*/) -> Result<Arc<accounts::GlobalAccount>, anyhow::Error> {
    // let global = constants::global_constants::GLOBAL_ACCOUNT;
    // if let Some(account) = ACCOUNT_CACHE.read().await.get(&global) {
    //     return Ok(account.clone());
    // }

    let global_account = accounts::GlobalAccount::new();

    // let account = rpc.get_account(&global).await?;
    // let global_account = bincode::deserialize::<accounts::GlobalAccount>(&account.data)?;
    let global_account = Arc::new(global_account);

    // ACCOUNT_CACHE.write().await.insert(global, global_account.clone());
    Ok(global_account)
}

#[inline]
pub async fn get_initial_buy_price(global_account: &Arc<accounts::GlobalAccount>, amount_sol: u64) -> Result<u64, anyhow::Error> {
    let buy_amount = global_account.get_initial_buy_price(amount_sol);
    Ok(buy_amount)
}

#[inline]
pub async fn get_bonding_curve_account(
    rpc: &SolanaRpcClient,
    mint: &Pubkey,
) -> Result<(Arc<accounts::BondingCurveAccount>, Pubkey), anyhow::Error> {
    let bonding_curve_pda = get_bonding_curve_pda(mint)
        .ok_or(anyhow!("Bonding curve not found"))?;

    let account = rpc.get_account(&bonding_curve_pda).await?;
    if account.data.is_empty() {
        return Err(anyhow!("Bonding curve not found"));
    }

    let bonding_curve = Arc::new(bincode::deserialize::<accounts::BondingCurveAccount>(&account.data)?);

    Ok((bonding_curve, bonding_curve_pda))
}

#[inline]
pub async fn get_bonding_curve_account_v2(
    rpc: &SolanaRpcClient,
    mint: &Pubkey,
) -> Result<(Arc<PumpfunBondingCurveAccount>, Pubkey), anyhow::Error> {
    let bonding_curve_pda = get_bonding_curve_pda(mint)
        .ok_or(anyhow!("Bonding curve not found"))?;

    let account = rpc.get_account(&bonding_curve_pda).await?;
    if account.data.is_empty() {
        return Err(anyhow!("Bonding curve not found"));
    }

    let bonding_curve = solana_sdk::borsh1::try_from_slice_unchecked::<PumpfunBondingCurveAccount>(&account.data)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize bonding curve account: {}", e))?;

    Ok((Arc::new(bonding_curve), bonding_curve_pda))
}

#[inline]
pub fn get_buy_token_amount(
    bonding_curve_account: &BondingCurveAccount,
    buy_sol_cost: u64,
    slippage_basis_points: Option<u64>,
) -> anyhow::Result<(u64, u64)> {
    let buy_token = bonding_curve_account.get_buy_price(buy_sol_cost).map_err(|e| anyhow!(e))?;

    let max_sol_cost = calculate_with_slippage_buy(buy_sol_cost, slippage_basis_points.unwrap_or(DEFAULT_SLIPPAGE));

    Ok((buy_token, max_sol_cost))
}

pub fn get_buy_token_amount_from_sol_amount(
    bonding_curve: &BondingCurveAccount,
    amount: u64,
) -> u64 {
    if amount == 0 {
        return 0;
    }

    if bonding_curve.virtual_token_reserves == 0 {
        return 0;
    }

    let total_fee_basis_points = FEE_BASIS_POINTS
        + if bonding_curve.creator != Pubkey::default() {
            CREATOR_FEE
        } else {
            0
        };

    // 转为 u128 防止溢出
    let amount_128 = amount as u128;
    let total_fee_basis_points_128 = total_fee_basis_points as u128;
    let input_amount = amount_128
        .checked_mul(10_000)
        .unwrap()
        .checked_div(total_fee_basis_points_128 + 10_000)
        .unwrap();

    let virtual_token_reserves = bonding_curve.virtual_token_reserves as u128;
    let virtual_sol_reserves = bonding_curve.virtual_sol_reserves as u128;
    let real_token_reserves = bonding_curve.real_token_reserves as u128;

    let denominator = virtual_sol_reserves + input_amount;

    let tokens_received = input_amount
        .checked_mul(virtual_token_reserves)
        .unwrap()
        .checked_div(denominator)
        .unwrap();

    tokens_received.min(real_token_reserves) as u64
}


#[inline]
pub async fn init_bonding_curve_account(
    mint: &Pubkey,
    dev_buy_token: u64,
    dev_sol_cost: u64,
    creator: Pubkey,
) -> Result<Arc<BondingCurveAccount>, anyhow::Error> {
    let bonding_curve = BondingCurveAccount::new(mint, dev_buy_token, dev_sol_cost, creator);
    let bonding_curve = Arc::new(bonding_curve);
    Ok(bonding_curve)
}

#[inline]
pub fn get_buy_amount_with_slippage(amount_sol: u64, slippage_basis_points: Option<u64>) -> u64 {
    let slippage = slippage_basis_points.unwrap_or(DEFAULT_SLIPPAGE);
    amount_sol + (amount_sol * slippage / 10000)
}

#[inline]
pub fn get_token_price(virtual_sol_reserves: u64, virtual_token_reserves: u64) -> f64 {
    let v_sol = virtual_sol_reserves as f64 / 100_000_000.0;
    let v_tokens = virtual_token_reserves as f64 / 100_000.0;
    v_sol / v_tokens
}

#[inline]
pub fn get_buy_price(amount: u64, trade_info: &TradeInfo) -> u64 {
    if amount == 0 {
        return 0;
    }

    let n: u128 = (trade_info.virtual_sol_reserves as u128) * (trade_info.virtual_token_reserves as u128);
    let i: u128 = (trade_info.virtual_sol_reserves as u128) + (amount as u128);
    let r: u128 = n / i + 1;
    let s: u128 = (trade_info.virtual_token_reserves as u128) - r;
    let s_u64 = s as u64;
    
    s_u64.min(trade_info.real_token_reserves)
}

#[inline]
pub fn calculate_with_slippage_buy(amount: u64, basis_points: u64) -> u64 {
    amount + (amount * basis_points) / 10000
}

#[inline]
pub fn calculate_with_slippage_sell(amount: u64, basis_points: u64) -> u64 {
    amount - (amount * basis_points) / 10000
}
