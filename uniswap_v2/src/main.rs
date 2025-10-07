use alloy::{
    primitives::{Address, U256, address},
    providers::{Provider, ProviderBuilder, WsConnect},
    rpc::types::{BlockNumberOrTag, Filter},
    sol,
    sol_types::SolEvent,
};
use anyhow::Result;
use chrono::DateTime;
use futures_util::StreamExt;

const RPC_URL: &str = "wss://mainnet.gateway.tenderly.co";

const ADDRESS: Address = address!("0xeae14c74ebe152da6dc58adfe383afcc342c78fa");

sol! {
    #[sol(rpc)]
    contract UniswapV2Pair {
        // Sync event - reserve changes after each swap
        event Sync(uint112 reserve0, uint112 reserve1);

        // Swap event - for volume calculation
        event Swap(
            address indexed sender,
            uint amount0In,
            uint amount1In,
            uint amount0Out,
            uint amount1Out,
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

#[tokio::main]
async fn main() -> Result<()> {
    // let provider = ProviderBuilder::new().connect(RPC_URL).await?;
    let ws = WsConnect::new(RPC_URL);
    let provider = ProviderBuilder::new().connect_ws(ws).await?;

    let pair_contract = UniswapV2Pair::new(ADDRESS, &provider);

    let token0_addr = pair_contract.token0().call().await?;
    let token1_addr = pair_contract.token1().call().await?;

    let token0_contract = ERC20::new(token0_addr, &provider);
    let token1_contract = ERC20::new(token1_addr, &provider);

    let token0_decimals = token0_contract.decimals().call().await?;
    let token1_decimals = token1_contract.decimals().call().await?;

    let token0_symbol = token0_contract.symbol().call().await?;
    let token1_symbol = token1_contract.symbol().call().await?;

    let reserves = pair_contract.getReserves().call().await?;
    let reserve0 = reserves.reserve0;
    let reserve1 = reserves.reserve1;

    let token0_per_token1 = calculate_price_v2(
        reserve0.to(),
        reserve1.to(),
        token0_decimals,
        token1_decimals,
    );

    println!(
        "1 {} = {:.10} {}",
        token0_symbol, token0_per_token1, token1_symbol
    );

    let token1_per_token0 = 1.0 / token0_per_token1;

    println!(
        "1 {} = {:.10} {}",
        token1_symbol, token1_per_token0, token0_symbol
    );

    /*
    let filter = Filter::new()
        .address(ADDRESS)
        .event(UniswapV2Pair::Sync::SIGNATURE)
        .from_block(BlockNumberOrTag::Latest);

    let sub = provider.subscribe_logs(&filter).await?;
    let mut stream = sub.into_stream();

    while let Some(log) = stream.next().await {
        tokio::spawn(async move {
            let sync = UniswapV2Pair::Sync::decode_log_data(log.data()).unwrap();
            println!("{:?}", sync.reserve0);
            println!("{:?}", sync.reserve1);
        });
    }
    */

    let filter = Filter::new()
        .address(ADDRESS)
        .event(UniswapV2Pair::Swap::SIGNATURE)
        .from_block(BlockNumberOrTag::Latest);

    let sub = provider.subscribe_logs(&filter).await?;
    let mut stream = sub.into_stream();
    while let Some(log) = stream.next().await {
        let token0_symbol_clone = token0_symbol.clone();
        let token1_symbol_clone = token1_symbol.clone();

        let pair = format!("{}-{}", token0_symbol_clone, token1_symbol_clone);

        let block_timestamp = log
            .block_timestamp
            .unwrap_or(chrono::Utc::now().timestamp() as u64);

        let timestamp = DateTime::from_timestamp(block_timestamp as i64, 0).unwrap();

        tokio::spawn(async move {
            let swap = UniswapV2Pair::Swap::decode_log_data(log.data()).unwrap();

            if swap.amount0In > U256::ZERO && swap.amount1Out > U256::ZERO {
                // BUY: Sending token0 to get token1
                let amount_in = format_token_amount(swap.amount0In, token0_decimals);
                let amount_out = format_token_amount(swap.amount1Out, token1_decimals);
                println!(
                    "{} - {}: BUY | {} {} → {} {}",
                    timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                    pair,
                    amount_in,
                    token0_symbol_clone,
                    amount_out,
                    token1_symbol_clone,
                );
            } else if swap.amount1In > U256::ZERO && swap.amount0Out > U256::ZERO {
                // SELL: Sending token1 to get token0
                let amount_in = format_token_amount(swap.amount1In, token1_decimals);
                let amount_out = format_token_amount(swap.amount0Out, token0_decimals);
                println!(
                    "{} - {}: SELL | {} {} → {} {}",
                    timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                    pair,
                    amount_in,
                    token1_symbol_clone,
                    amount_out,
                    token0_symbol_clone,
                );
            }
        });
    }

    Ok(())
}

// Format token amount with decimals
fn format_token_amount(amount: U256, decimals: u8) -> String {
    let divisor = 10_f64.powi(decimals as i32);
    let amount_f64 = amount.to::<u128>() as f64 / divisor;

    // Format with appropriate precision
    if amount_f64 >= 1.0 {
        format!("{:.4}", amount_f64)
    } else if amount_f64 >= 0.0001 {
        format!("{:.8}", amount_f64)
    } else {
        format!("{:.12}", amount_f64)
    }
}

fn calculate_price_v2(
    reserve0: u128,
    reserve1: u128,
    token0_decimals: u8,
    token1_decimals: u8,
) -> f64 {
    // In Uniswap V2, price = reserve1 / reserve0 (token1 per token0)
    // But we want token0 per token1, so we need reserve0 / reserve1
    let reserve0_f64 = reserve0 as f64;
    let reserve1_f64 = reserve1 as f64;

    // Calculate the raw price ratio
    let price_ratio = reserve1_f64 / reserve0_f64;

    // Adjust for decimal differences
    let price_adjusted =
        price_ratio * 10_f64.powi((token0_decimals as i32) - (token1_decimals as i32));

    price_adjusted
}

/*
SELECT
    timestamp,
    first(price) AS open,
    last(price) AS close,
    min(price) AS low,
    max(price) AS high,
    sum(amount) AS volume
FROM trades
WHERE symbol = 'BTC-USD' AND timestamp > dateadd('d', -1, now())
SAMPLE BY 15m ALIGN TO CALENDAR;

"timestamp","open","close","low","high","volume"
"2025-10-06T13:15:00.000000Z",124852.9,124860.0,124788.9,124932.1,39.7377852999999
"2025-10-06T13:30:00.000000Z",124860.0,124571.4,124423.7,124982.6,137.50690133999436
"2025-10-06T13:45:00.000000Z",124572.0,125036.0,124400.2,125079.0,136.80196229999822
"2025-10-06T14:00:00.000000Z",125036.0,124980.7,124826.6,125348.0,210.32002145000158
"2025-10-06T14:15:00.000000Z",124980.7,124850.0,124545.3,125139.2,101.01958531000047
"2025-10-06T14:30:00.000000Z",124860.0,124742.7,124609.0,125062.5,66.06626907999971
"2025-10-06T14:45:00.000000Z",124742.8,124521.7,124300.0,124899.8,81.83028521000014
"2025-10-06T15:00:00.000000Z",124522.8,124733.0,124510.1,124747.9,22.901160430000182
"2025-10-06T15:15:00.000000Z",124733.0,125175.0,124731.0,125420.6,114.79884345000008
"2025-10-06T15:30:00.000000Z",125175.0,125088.8,124972.6,125290.1,62.331448020001325

*/
