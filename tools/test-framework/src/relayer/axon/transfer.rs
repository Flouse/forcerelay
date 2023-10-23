//! Implement ibc transfer with MockTransfer contract

use std::{path::Path, sync::Arc};

use crate::{prelude::*, types::axon::DeployedContracts};
use ethers::{
    prelude::{k256::ecdsa::SigningKey, Wallet, *},
    providers::{Middleware, Provider, Ws},
};
use eyre::eyre;
use ibc_relayer::{
    chain::axon::utils::ibc_event_from_ibc_handler_event, event::IbcEventWithHeight,
    ibc_contract::OwnableIBCHandlerEvents, keyring::Secp256k1KeyPair,
};
use ibc_relayer_types::{core::ics04_channel::packet::Packet, events::IbcEvent, Height};

abigen!(
    TransferContract,
    r"[
        function mint(address account, string memory denom, uint256 amount) external
        function sendTransfer(string calldata denom, uint64 amount, address receiver, string calldata sourcePort, string calldata sourceChannel, uint64 timeoutHeight) external
        function transferFrom(address sender, address receiver, string memory denom, uint256 amount) external
        function denomTokenContract(string denom) returns(address)
        function getEscrowAddress(string memory sourceChannel) external view returns (address)
    ]"
);

abigen!(
    ERC20,
    r"[
        function totalSupply() external view returns (uint256)
        function balanceOf(address account) external view returns (uint256)
        function transfer(address to, uint256 amount) external returns (bool)
        function allowance(address owner, address spender) external view returns (uint256)
        function approve(address spender, uint256 amount) external returns (bool)
        function transferFrom(address from, address to, uint256 amount) external returns (bool)
    ]"
);

async fn new_contract(
    client: Provider<Ws>,
    key_pair: &Secp256k1KeyPair,
    address: H160,
) -> eyre::Result<TransferContract<SignerMiddleware<Provider<Ws>, Wallet<SigningKey>>>> {
    let chain_id: u64 = client.get_chainid().await?.as_u64();
    let wallet = key_pair.clone().into_ether_wallet().with_chain_id(chain_id);
    let client = Arc::new(SignerMiddleware::new(client.clone(), wallet));
    Ok(TransferContract::new(address, client))
}

async fn new_erc20(
    client: Provider<Ws>,
    key_pair: &Secp256k1KeyPair,
    address: H160,
) -> eyre::Result<ERC20<SignerMiddleware<Provider<Ws>, Wallet<SigningKey>>>> {
    let chain_id: u64 = client.get_chainid().await?.as_u64();
    let wallet = key_pair.clone().into_ether_wallet().with_chain_id(chain_id);
    let client = Arc::new(SignerMiddleware::new(client.clone(), wallet));
    Ok(ERC20::new(address, client))
}

pub fn read_deployed_contracts<P: AsRef<Path>>(chain_dir: P) -> Result<DeployedContracts, Error> {
    const AXON_CONTRACTS_CONFIG_PATH: &str = "deployed_contracts.toml";

    let path = chain_dir.as_ref().join(AXON_CONTRACTS_CONFIG_PATH);
    let content = std::fs::read_to_string(path)?;
    let c = toml::from_str(&content).map_err(|err| eyre::eyre!(err))?;
    Ok(c)
}

/// ibc token transfer
pub async fn ibc_token_transfer<SrcChain, DstChain>(
    websocket_addr: String,
    home_path: String,
    port_id: &TaggedPortIdRef<'_, SrcChain, DstChain>,
    channel_id: &TaggedChannelIdRef<'_, SrcChain, DstChain>,
    sender: &MonoTagged<SrcChain, &crate::types::wallet::Wallet>,
    recipient: &MonoTagged<DstChain, &WalletAddress>,
    token: &TaggedTokenRef<'_, SrcChain>,
    timeout: Option<Duration>,
) -> Result<Packet, Error> {
    // we set ws port on the next port of rpc port in `ibc-test/src/framework/bootstrap/node.rs`
    let client = Provider::connect(websocket_addr)
        .await
        .map_err(|err| eyre!(err))?;
    // get contract addresses
    let deployed = read_deployed_contracts(&home_path).expect("failed to fetch deployed contracts");
    let transfer_address = deployed.transfer_contract_address;
    let ibc_handler_address = deployed.contract_address;
    let contract = new_contract(client.clone(), &sender.value().key, transfer_address).await?;

    let receiver = {
        let slice = hex::decode(recipient.value().as_str().trim_start_matches("0x"))
            .map_err(|err| eyre!(err))?;
        H160::from_slice(&slice)
    };
    let denom = token.denom().value().to_string();
    let amount = token.amount().0.as_u64();
    let timeout_height = timeout.map(|d| d.as_secs() / 8).unwrap_or_default();
    // ERC20 token approving
    {
        // Parse ERC20 address
        let token_address = H160::from_slice(&hex::decode(denom.trim_start_matches("0x")).unwrap());
        let token = new_erc20(client.clone(), &sender.value().key, token_address).await?;
        // approve
        let tx = token.approve(transfer_address, amount.into());
        let pending_tx = tx.send().await.unwrap();
        pending_tx.await.unwrap().unwrap();
    }
    // ICS sendTransfer
    let tx = contract.send_transfer(
        denom,
        amount,
        receiver,
        port_id.to_string(),
        channel_id.to_string(),
        timeout_height,
    );
    let pending_tx = tx.send().await.map_err(|err| eyre!(err))?;
    let receipt_opt = pending_tx.await.map_err(|err| eyre!(err))?;
    let receipt = receipt_opt.ok_or(eyre!("axon send ibc transfer no receipt"))?;

    let block_number = receipt.block_number.unwrap().as_u64();
    let tx_hash = receipt.transaction_hash.into();
    // check packet is sent
    let ibc_logs: Vec<Log> = receipt
        .logs
        .into_iter()
        .filter(|log| log.address == ibc_handler_address)
        .collect();
    let events = fetch_all_ibc_events_from_tx_logs(block_number, tx_hash, &ibc_logs)?;
    let packet = events
        .into_iter()
        .find_map(|event| match event.event {
            IbcEvent::SendPacket(ev) => Some(ev.packet),
            _ => None,
        })
        .ok_or_else(|| eyre!("failed to find send packet event"))?;
    Ok(packet)
}

pub fn fetch_all_ibc_events_from_tx_logs(
    block_number: u64,
    tx_hash: [u8; 32],
    logs: &[Log],
) -> Result<Vec<IbcEventWithHeight>, eyre::Error> {
    let height = Height::from_noncosmos_height(block_number);
    // check send packet event
    let events = logs
        .iter()
        .filter_map(|log| {
            let event =
                OwnableIBCHandlerEvents::decode_log(&log.clone().into()).expect("decode log");
            ibc_event_from_ibc_handler_event(height, tx_hash, event).transpose()
        })
        .collect::<Result<_, eyre::Error>>()?;
    Ok(events)
}
