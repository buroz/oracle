use anyhow::Result;
use alloy::{
    primitives::{address, Address, Uint, U256}, 
    providers::ProviderBuilder, 
    sol 
};

const RPC_URL: &str = "https://mainnet.gateway.tenderly.co";

const POOL_ADDRESS: Address = address!("0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8");

sol! {
    #[sol(rpc)] 
    contract UniswapV3Pool {
        struct Slot0 {
            // the current price
            uint160 sqrtPriceX96;
            // the current tick
            int24 tick;
            // the most-recently updated index of the observations array
            uint16 observationIndex;
            // the current maximum number of observations that are being stored
            uint16 observationCardinality;
            // the next maximum number of observations to store, triggered in observations.write
            uint16 observationCardinalityNext;
            // the current protocol fee as a percentage of the swap fee taken on withdrawal
            // represented as an integer denominator (1/x)%
            uint8 feeProtocol;
            // whether the pool is locked
            bool unlocked;
        }
    
        function token0() external view returns (address);
        function token1() external view returns (address);

        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
    }

    #[sol(rpc)]
    contract ERC20 {
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let provider = ProviderBuilder::new().connect(RPC_URL).await?;

    let pool_contract = UniswapV3Pool::new(POOL_ADDRESS, &provider); 

    let token0_addr = pool_contract.token0().call().await?;
    let token1_addr = pool_contract.token1().call().await?;

    let token0_contract = ERC20::new(token0_addr, &provider);
    let token1_contract = ERC20::new(token1_addr, &provider);

    let token0_decimals = token0_contract.decimals().call().await?;
    let token1_decimals = token1_contract.decimals().call().await?;

    let token0_symbol = token0_contract.symbol().call().await?;
    let token1_symbol = token1_contract.symbol().call().await?;

    let slot0 = pool_contract.slot0().call().await?;
    let sqrt_price_x96 = slot0.sqrtPriceX96;

    let token0_per_token1 = calculate_price(sqrt_price_x96, token0_decimals, token1_decimals);
    println!("1 {} = {:.10} {}", token0_symbol, token0_per_token1, token1_symbol);

    let token1_per_token0 = 1.0 / token0_per_token1;
    println!("1 {} = {:.10} {}", token1_symbol, token1_per_token0, token0_symbol);

    Ok(())
}

fn calculate_price(sqrt_price_x96: Uint<160, 3>, token0_decimals: u8, token1_decimals: u8) -> f64 {
    let sqrt_price = U256::from(sqrt_price_x96);
    
    // price = (sqrtPriceX96 / 2^96)^2
    let q_96 = U256::from(2).pow(U256::from(96));
    let sqrt_price_f64 = sqrt_price.to::<u128>() as f64;
    let q_96_f64 = q_96.to::<u128>() as f64;
    let price_ratio = (sqrt_price_f64 / q_96_f64).powi(2);

    let price_adjusted = price_ratio * 10_f64.powi((token0_decimals as i32) - (token1_decimals as i32));

    price_adjusted
}
