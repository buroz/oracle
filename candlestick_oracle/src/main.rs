use alloy::{
    primitives::{address, keccak256, Address, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{Filter, Log},
    sol,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

const RPC_URL: &str = "https://mainnet.gateway.tenderly.co";

// DAI/WETH Uniswap V2 pair address
const PAIR_ADDRESS: Address = address!("0xc4704f13d5e08b27b039d53873e813dd2fad99d9");

sol! {
    #[sol(rpc)]
    contract UniswapV2Pair {
        // Sync event - reserve changes after each swap
        event Sync(uint112 reserve0, uint112 reserve1);

        // Swap event - for volume calculation
        event Swap(
            address indexed sender,
            uint256 amount0In,
            uint256 amount1In,
            uint256 amount0Out,
            uint256 amount1Out,
            address indexed to
        );

        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }

    #[sol(rpc)]
    contract ERC20 {
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct CandlestickData {
    timestamp: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

#[derive(Debug, Clone)]
struct PriceData {
    timestamp: DateTime<Utc>,
    price: f64,
    volume_usd: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let provider = ProviderBuilder::new().connect_http(RPC_URL.parse()?);

    // Get token info first
    let pair_contract = UniswapV2Pair::new(PAIR_ADDRESS, &provider);

    let token0_addr = pair_contract.token0().call().await?;
    let token1_addr = pair_contract.token1().call().await?;

    let token0_contract = ERC20::new(token0_addr, &provider);
    let token1_contract = ERC20::new(token1_addr, &provider);

    let token0_symbol = token0_contract.symbol().call().await?;
    let token1_symbol = token1_contract.symbol().call().await?;

    let token0_decimals = token0_contract.decimals().call().await?;
    let token1_decimals = token1_contract.decimals().call().await?;

    println!("💱 Trading Pair: {} / {}", token0_symbol, token1_symbol);

    // Get recent blocks for historical data
    let latest_block = provider.get_block_number().await?;
    let from_block = latest_block.saturating_sub(2000); // Last ~2000 blocks (~8 hours)

    println!(
        "🔍 Scanning blocks {} to {} for events",
        from_block, latest_block
    );

    // Fetch historical candlestick data
    let price_data = get_historical_price_data(
        &provider,
        PAIR_ADDRESS,
        from_block,
        latest_block,
        token0_decimals,
        token1_decimals,
    )
    .await?;

    println!("📈 Found {} price data points", price_data.len());

    let interval_minutes = 1;

    // Create 5-minute candlesticks
    let candlesticks = create_candlesticks(price_data, interval_minutes).await?;

    println!(
        "🕯️  Generated {} candlesticks ({} minute intervals)",
        candlesticks.len(),
        interval_minutes
    );
    println!(
        "🕯️  Generated {} candlesticks ({} minute intervals)",
        candlesticks.len(),
        interval_minutes
    );

    // Output as JSON
    let json_output = serde_json::to_string_pretty(&candlesticks)?;

    // Save to file
    let filename = "candlestick_data.json";
    fs::write(filename, &json_output)?;
    println!("💾 Data saved to {}", filename);

    println!("\n📋 Candlestick Data (JSON):");
    println!("{}", json_output);

    Ok(())
}

async fn get_historical_price_data(
    provider: &impl Provider,
    pair_address: Address,
    from_block: u64,
    to_block: u64,
    token0_decimals: u8,
    token1_decimals: u8,
) -> Result<Vec<PriceData>> {
    // Sync event signature: keccak256("Sync(uint112,uint112)")
    let sync_signature = keccak256("Sync(uint112,uint112)");

    let filter = Filter::new()
        .address(pair_address)
        .event_signature(sync_signature)
        .from_block(from_block)
        .to_block(to_block);

    let logs = provider.get_logs(&filter).await?;
    let mut price_data = Vec::new();

    println!("🔄 Processing {} Sync events...", logs.len());

    for log in logs {
        if let Ok(data) = parse_sync_event(&log, provider, token0_decimals, token1_decimals).await {
            price_data.push(data);
        }
    }

    // Sort by timestamp
    price_data.sort_by_key(|d| d.timestamp);

    Ok(price_data)
}

async fn parse_sync_event(
    log: &Log,
    provider: &impl Provider,
    token0_decimals: u8,
    token1_decimals: u8,
) -> Result<PriceData> {
    // Parse event data: Sync(uint112 reserve0, uint112 reserve1)
    let data = &log.data().data;

    // Each uint112 takes 32 bytes in event data (padded)
    let reserve0 = U256::from_be_slice(&data[0..32]).to::<u128>();
    let reserve1 = U256::from_be_slice(&data[32..64]).to::<u128>();

    // Get block timestamp
    let block_number = log.block_number.unwrap_or_default();
    let block = provider.get_block_by_number(block_number.into()).await?;
    let timestamp = DateTime::from_timestamp(block.unwrap().header.timestamp as i64, 0)
        .unwrap_or_else(|| Utc::now());

    // Calculate price (token0 per token1)
    let price = calculate_price_v2(reserve0, reserve1, token0_decimals, token1_decimals);

    // Estimate volume (simplified - in real implementation you'd track swap events)
    let volume_usd =
        estimate_volume_from_reserves(reserve0, reserve1, token0_decimals, token1_decimals);

    Ok(PriceData {
        timestamp,
        price,
        volume_usd,
    })
}

fn calculate_price_v2(
    reserve0: u128,
    reserve1: u128,
    token0_decimals: u8,
    token1_decimals: u8,
) -> f64 {
    if reserve0 == 0 || reserve1 == 0 {
        return 0.0;
    }

    let reserve0_f64 = reserve0 as f64;
    let reserve1_f64 = reserve1 as f64;

    // Price = reserve1 / reserve0 (token1 per token0)
    // But we want token0 per token1, so we need reserve0 / reserve1
    let price_ratio = reserve1_f64 / reserve0_f64;

    // Adjust for decimal differences
    let price_adjusted =
        price_ratio * 10_f64.powi((token0_decimals as i32) - (token1_decimals as i32));

    price_adjusted
}

fn estimate_volume_from_reserves(
    reserve0: u128,
    reserve1: u128,
    token0_decimals: u8,
    token1_decimals: u8,
) -> f64 {
    // Simplified volume estimation based on reserve size
    // In real implementation, you'd sum up actual swap volumes
    let reserve0_normalized = reserve0 as f64 / 10_f64.powi(token0_decimals as i32);
    let reserve1_normalized = reserve1 as f64 / 10_f64.powi(token1_decimals as i32);

    // Rough estimate: assume 0.1% of reserves traded per block
    (reserve0_normalized + reserve1_normalized) * 0.001
}

async fn create_candlesticks(
    price_data: Vec<PriceData>,
    interval_minutes: u64,
) -> Result<Vec<CandlestickData>> {
    let mut intervals: BTreeMap<i64, Vec<PriceData>> = BTreeMap::new();

    // Group price data by time intervals
    for data in price_data {
        let interval_start = (data.timestamp.timestamp() / (interval_minutes as i64 * 60))
            * (interval_minutes as i64 * 60);
        intervals.entry(interval_start).or_default().push(data);
    }

    let mut candlesticks = Vec::new();

    // Create candlestick for each interval
    for (timestamp, mut interval_data) in intervals {
        if interval_data.is_empty() {
            continue;
        }

        // Sort by timestamp within interval
        interval_data.sort_by_key(|d| d.timestamp);

        let prices: Vec<f64> = interval_data.iter().map(|d| d.price).collect();
        let total_volume: f64 = interval_data.iter().map(|d| d.volume_usd).sum();

        let open = *prices.first().unwrap_or(&0.0);
        let close = *prices.last().unwrap_or(&0.0);
        let high = prices.iter().fold(0.0f64, |a, &b| a.max(b));
        let low = prices.iter().fold(f64::INFINITY, |a, &b| a.min(b));

        let candlestick = CandlestickData {
            timestamp: timestamp * 1000, // Convert to milliseconds
            open: format!("{:.32}", open),
            high: format!("{:.32}", high),
            low: format!("{:.32}", low),
            close: format!("{:.32}", close),
            volume: format!("{:.16}", total_volume),
        };

        candlesticks.push(candlestick);
    }

    Ok(candlesticks)
}
