// Use k8s config to find cluster
// Check it's Mojaloop?
// Port-forward if appropriate?

// Parameters:
// - optionally create puppeteer server in cluster
//   - puppeteer server config
// - k8s config file
// - puppeteer endpoint
//
// Commands:
// - transfers
//   - list
//   - prepare
//   - fulfil
//   - prepare-fulfil
// - participants
//   - create
//   - enable/disable
//   - accounts
//     - create
//     - enable/disable
//   - configure endpoints
// - settlements

use mojaloop_api::{
    common::{to_hyper_request, Response, MlApiErr},
    central_ledger::participants::{HubAccountType, GetParticipants, Limit, LimitType, PostParticipant, InitialPositionAndLimits, GetDfspAccounts, HubAccount, PostHubAccount, DfspAccounts, PostInitialPositionAndLimits, NewParticipant},
};
use fspiox_api::{
    build_post_quotes, build_transfer_prepare, FspiopRequestBody,
    common::{Amount, Currency, FspId, ErrorResponse, CorrelationId},
    transfer,
    quote,
};

extern crate clap;
use clap::Clap;

use futures::{StreamExt, TryStreamExt};
use thiserror::Error;

use hyper::http::{Request, StatusCode};
use hyper::{client::conn::Builder, Body};

use kube::{
    api::{Api, DeleteParams, ListParams, PostParams, WatchEvent},
    Client, ResourceExt,
};

use tokio::io::AsyncWriteExt;

use cli_table::{print_stdout, Cell, Table};

#[derive(Clap)]
#[clap(
    setting = clap::AppSettings::ArgRequiredElseHelp,
    version = clap::crate_version!(),
    name = "Mojaloop CLI",
    rename_all = "kebab",
)]
struct Opts {
    // TODO: overall timeout? Probably. Remember how annoying `kubectl wait` is, with its
    // per-request timeout, meaning that you could wait up to n*timeout to wait for n items.
    /// Per-request timeout. A single command may make multiple requests.
    #[clap(short, long, default_value = "30")]
    timeout: u8,

    /// Location of the kubeconfig file to use
    #[clap(short, long)]
    kubeconfig: Option<String>,

    // TODO: all namespace option? Don't have a reserved "all" argument i.e. --namespace=all,
    // because someone could call their real namespace "all". Probably try to go with common k8s
    // flags for this, perhaps -A and --all-namespaces (check those are correct).
    /// Namespace in which to find the Mojaloop deployment. Defaults to the default namespace in
    /// your kubeconfig, or "default".
    #[clap(short, long)]
    namespace: Option<String>,

    /// Produce all output as json
    #[clap(short, long)]
    json: bool,

    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Clap)]
enum SubCommand {
    /// Create, read, update, and upsert a single switch participant
    Participant(Participant),
    /// Create, read, enable, and disable accounts
    Accounts(Accounts),
    /// List participants
    Participants(Participants),
    /// Hub functions
    Hub(Hub),
    /// Execute transfers
    ///
    /// Note that this implementation is rather simplistic and does not verify that actions it
    /// takes are successful, because it would require some manner of FSP implementation to receive
    /// forwarded transfer prepare/fulfil requests from the switch. Therefore, this functionality
    /// generally expects the user to have deployed simulators to receive transfer prepare and
    /// fulfil messages.
    ///
    /// For a more complete, but more
    /// complex implementation, use the puppet subcommand to this utility.
    #[clap(alias = "tx")]
    Transfer(Transfer),
    /// Create quotes
    Quote(Quote),
    // /// Complex behaviours and scenarios that require a component deployed to the cluster to
    // /// simulate participants.
    // Puppet(Puppet),
}

#[derive(Clap)]
struct Quote {
    #[clap(subcommand)]
    subcmd: QuoteSubCommand,
}

#[derive(Clap)]
enum QuoteSubCommand {
    #[clap(alias = "new")]
    Create(QuoteCreate),
}

#[derive(Clap)]
struct QuoteCreate {
    #[clap(index = 1, required = true)]
    from: FspId,
    #[clap(index = 2, required = true)]
    to: FspId,
    #[clap(index = 3, required = true)]
    currency: Currency,
    // TODO: take multiple
    #[clap(index = 4, required = true)]
    amount: Amount,
}

#[derive(Clap)]
struct Puppet {
    #[clap(short,long)]
    /// Create any participants, accounts etc. required by this command where they do not exist.
    ///
    /// If participants used by this command do exist, this utility will exit with an error before
    /// taking any action. To use existing participants, if they exist, and create them if they do
    /// not exist, combine this flag with the --hijack flag.
    create: bool,
    /// Disable participants and accounts created by this command.
    #[clap(short,long)]
    cleanup_created: bool,
    /// Disable any participants and accounts used by this command once the command has been executed
    ///
    /// Disable participants and accounts used by this command. Warning: this will disable
    /// participants and accounts that existed _before_ this command was called.
    #[clap(short,long)]
    cleanup: bool,
    /// Take control of any participants specified in this command
    ///
    /// This will temporarily reroute all endpoints for any participants used in this command to
    /// puppeteer. This means the entity normally configured to receive FSPIOP requests at these
    /// endpoints will not receive them. Endpoints will be restored after the command completes.
    #[clap(short,long)]
    hijack: bool,
    #[clap(subcommand)]
    subcmd: PuppetSubCommand,
}

#[derive(Clap)]
enum PuppetSubCommand {
    Transfer(PuppetTransfer),
}

#[derive(Clap)]
struct PuppetTransfer {
    #[clap(subcommand)]
    subcmd: PuppetTransferSubCommand,
}

#[derive(Clap)]
enum PuppetTransferSubCommand {
    /// Create a transfer from an existing quote
    FromQuote(PuppetTransferFromQuote),
}

#[derive(Clap)]
struct PuppetTransferFromQuote {}

#[derive(Clap)]
struct Transfer {
    #[clap(subcommand)]
    subcmd: TransferSubCommand,
}

#[derive(Clap)]
enum TransferSubCommand {
    /// Prepare (POST) a transfer.
    ///
    /// The transfer correlation ID will be generated and printed as output.
    #[clap(alias = "post")]
    Prepare(TransferPrepare),
    // /// Fulfil (PUT) a transfer.
    // ///
    // /// You'll probably want to use a correlation ID from a transfer prepare here.
    // #[clap(alias = "put")]
    // Fulfil(TransferFulfil),
    // /// Prepare and fulfil a transfer.
    // ///
    // /// This command has no way of determining whether a transfer prepare has been received by the
    // /// intended recipient. It therefore waits a configurable amount of time for this to occur
    // /// between sending prepares and fulfils.
    // PrepareFulfil(TransferPrepareFulfil),
}

#[derive(Clap)]
struct TransferPrepare {
    #[clap(subcommand)]
    subcmd: TransferPrepareSubCommand,
}

#[derive(Clap)]
enum TransferPrepareSubCommand {
    /// Prepare a transfer without an existing transaction ID
    New(TransferPrepareNew),
    /// Prepare a transfer from an existing quote transaction ID
    ///
    /// Note that we can't call GET /quotes here to fill the details of the 
    FromTransaction(TransferPrepareWithId),
}

#[derive(Clap)]
struct TransferPrepareWithId {
    // TODO: we probably should use quote::TransactionId here for correctness, but note that
    // TransactionId and TransferId should be totally interchangeable. Everywhere a TransactionId
    // is used, a TransferId should be accepted, and vice versa. Or not? E.g., an existing
    // TransferId should not be used for a quote request. This todo mostly exists as a reminder to
    // think about the implications of these types, and to perhaps modify the API types in
    // fspiox-api to match conclusions. I.e. we might actually want to say
    //   pub type TransactionId = transfer::TransferId
    #[clap(index = 1, required = true)]
    from: FspId,
    #[clap(index = 2, required = true)]
    to: FspId,
    #[clap(index = 3, required = true)]
    currency: Currency,
    // TODO: it might be possible to put these under flags or a subcommand or similar to allow
    // multiple. I.e. we might be able to say
    //   mojaloop-cli transfer prepare from-transaction payerfsp payeefsp XOF \
    //     send 100 e1f3c512-dd8e-4b5b-ad59-4e87bf97fcb8 \
    //     send 200 e1f3c512-dd8e-4b5b-ad59-4e87bf97fcb8 \
    //     ...
    // or similar
    #[clap(index = 4, required = true)]
    amount: Amount,
    #[clap(index = 5, required = true)]
    transfer_id: transfer::TransferId,
}

#[derive(Clap)]
struct TransferPrepareNew {
    #[clap(index = 1, required = true)]
    from: FspId,
    #[clap(index = 2, required = true)]
    to: FspId,
    #[clap(index = 3, required = true)]
    currency: Currency,
    // TODO: take multiple
    #[clap(index = 4, required = true)]
    amount: Amount,
}

#[derive(Clap)]
struct TransferFulfil {
    #[clap(index = 1, default_value = "COMMITTED")]
    state: transfer::TransferState,
    #[clap(index = 2, required = true, multiple = true)]
    id: CorrelationId,
}

#[derive(Clap)]
struct TransferPrepareFulfil {
    #[clap(index = 1, required = true)]
    from: FspId,
    #[clap(index = 2, required = true)]
    to: FspId,
    #[clap(index = 3, required = true)]
    currency: Currency,
    #[clap(index = 4, default_value = "COMMITTED")]
    state: transfer::TransferState,
    #[clap(index = 5, required = true, multiple = true)]
    amount: Amount,
}

#[derive(Clap)]
struct Hub {
    #[clap(subcommand)]
    subcmd: HubSubCommand,
}

#[derive(Clap)]
enum HubSubCommand {
    /// Create and read hub accounts
    Accounts(HubAccounts)
}

#[derive(Clap)]
struct HubAccounts {
    #[clap(subcommand)]
    subcmd: HubAccountsSubCommand,
}

#[derive(Clap)]
enum HubAccountsSubCommand {
    /// List hub accounts
    List,
    /// Create hub accounts
    Create(HubAccountsCreate),
    // TODO: upsert
}

#[derive(Clap, Debug)]
struct HubAccountsCreate {
    #[clap(subcommand)]
    subcmd: HubAccountsCreateSubCommand,
}

#[derive(Clap, Debug)]
enum HubAccountsCreateSubCommand {
    /// Create multilateral settlement accounts
    Sett(HubAccountsCreateOpts),
    /// Create reconciliation accounts
    Rec(HubAccountsCreateOpts),
    /// Create all hub account types with one command
    All(HubAccountsCreateOpts),
}

#[derive(Clap, Debug)]
struct HubAccountsCreateOpts {
    #[clap(index = 1, required = true, multiple = true)]
    currencies: Vec<Currency>,
}

#[derive(Clap)]
struct Participants {
    #[clap(subcommand)]
    subcmd: ParticipantsSubCommand,
}

#[derive(Clap)]
enum ParticipantsSubCommand {
    /// List participants
    List,
}

#[derive(Clap)]
struct Participant {
    #[clap(index = 1, required = true)]
    name: FspId,
    #[clap(subcommand)]
    subcmd: ParticipantSubCommand,
}

#[derive(Clap)]
enum ParticipantSubCommand {
    /// Modify participant account
    Accounts(ParticipantAccount),
    /// Create a participant
    Create(ParticipantCreate),
}

#[derive(Clap)]
struct ParticipantCreate {
    currency: Currency,
    #[clap(default_value = "10000")]
    ndc: u32,
    #[clap(default_value = "10000")]
    position: Amount,
}

#[derive(Clap)]
struct ParticipantAccount {
    #[clap(subcommand)]
    subcmd: ParticipantAccountsSubCommand,
}

#[derive(Clap)]
enum ParticipantAccountsSubCommand {
    /// Fund participant account
    Fund(ParticipantAccountFund),
    /// Upsert participant account
    Upsert(ParticipantAccountUpsert),
    /// List participant accounts
    List,
}

#[derive(Clap, Debug)]
struct ParticipantAccountFund {
    #[clap(index = 1, required = true)]
    currency: Currency,
    #[clap(subcommand)]
    subcmd: ParticipantAccountFundSubCommand,
}

#[derive(Clap, Debug)]
enum ParticipantAccountFundSubCommand {
    /// Process funds into the account.
    In(ParticipantAccountFundsPositive),
    /// Process funds out of the account.
    Out(ParticipantAccountFundsPositive),
    /// Fund a numeric amount. Positive: funds in. Negative: funds out. You'll likely need to
    /// provide the argument after --, thus: participant my_participant fund XOF num -- -100
    Num(ParticipantAccountFunds),
}

#[derive(Clap, Debug)]
struct ParticipantAccountFunds {
    amount: Amount,
}

#[derive(Clap, Debug)]
struct ParticipantAccountFundsPositive {
    // TODO: how to enforce this to be positive?
    amount: Amount,
}

#[derive(Clap, Debug)]
struct ParticipantAccountUpsert {
    #[clap(index = 1)]
    currency: Currency,
    #[clap(short, long, requires = "currency")]
    ndc: Option<Amount>,
    #[clap(short, long, requires = "currency")]
    position: Option<Amount>,
}

#[derive(Clap)]
struct Accounts {
    #[clap(subcommand)]
    subcmd: AccountsSubCommand,
}

#[derive(Clap)]
enum AccountsSubCommand {
    /// Create accounts
    Create(AccountsCreate),
}

#[derive(Clap, Debug)]
struct AccountsCreate {
    #[clap(index = 1)]
    participant_name: FspId,
    #[clap(index = 2)]
    currency: Currency,
}

// TODO: crate::Result (i.e. a result type for this crate, that uses this error type as its error
// type). Probably just call this type "Error"?
#[derive(Error, Debug)]
pub enum MojaloopCliError {
    #[error("Couldn't find central ledger admin pod")]
    CentralLedgerAdminPodNotFound,
    #[error("Unexpected central ledger pod manifest implementation")]
    UnexpectedCentralLedgerPodImplementation,
    #[error("Central ledger service container not found in pod")]
    CentralLedgerServiceContainerNotFound,
    #[error("Ports not present on central ledger service container")]
    CentralLedgerServicePortNotFound,
    #[error("Couldn't retrieve pod list from cluster: {0}")]
    ClusterConnectionError(kube::Error),
    #[error("Failed to send HTTP request to port-forwarded pod: {0}")]
    PortForwardConnectionError(String),
    #[error("Error parsing HTTP response from port-forwarded pod: {0}")]
    PortForwardResponseParseError(String),
    #[error("Expected HTTP response body but none was returned")]
    PortForwardResponseNoBody,
    #[error("Unhandled HTTP response from port-forwarded pod: {0}")]
    PortForwardUnhandledResponse(String),
    #[error("Mojaloop API error: {0}")]
    MojaloopApiError(ErrorResponse),
}

mod port_forward {
    use kube::{
        api::{Api, ListParams},
        Client,
    };
    use k8s_openapi::api::core::v1::Pod;
    use crate::MojaloopCliError;
    use std::convert::TryInto;

    async fn from_params(
        pods: &Api<Pod>,
        label: &str,
        container_name: &str,
        port: Port,
    ) -> anyhow::Result<(impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin)> {
        let lp = ListParams::default().labels(&label);
        let pod = pods
            .list(&lp).await.map_err(|e| MojaloopCliError::ClusterConnectionError(e))?
            .items.get(0).ok_or(MojaloopCliError::CentralLedgerAdminPodNotFound)?.clone();
        let pod_name = pod.metadata.name.clone().unwrap();
        let pod_port = pod
            .spec
            .ok_or(MojaloopCliError::UnexpectedCentralLedgerPodImplementation)?
            .containers.iter().find(|c| c.name == container_name)
            .ok_or(MojaloopCliError::CentralLedgerServiceContainerNotFound)?
            .ports.as_ref().ok_or(MojaloopCliError::CentralLedgerServicePortNotFound)?
            .iter()
            .find(|p| {
                match &port {
                    Port::Name(port_name) => p.name.as_ref().map_or(false, |name| name == port_name),
                    Port::Number(port_num) => p.container_port == *port_num,
                }
            })
            .ok_or(MojaloopCliError::CentralLedgerServicePortNotFound)?.clone();
        // Ok((pod_name, port.container_port.try_into().unwrap()))

        let mut pf = pods.portforward(
            &pod_name,
            &[pod_port.container_port.try_into().unwrap()]
        ).await?;
        let mut ports = pf.ports().unwrap();
        let result = ports[0].stream().unwrap();
        Ok(result)
    }

    pub enum Services {
        CentralLedger,
        MlApiAdapter,
        QuotingService,
    }

    enum Port {
        Name(String),
        Number(i32),
    }

    pub async fn get(namespace: &Option<String>, services: &[Services]) ->
        anyhow::Result<Vec<(impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin)>>
    {
        let client = Client::try_default().await?;
        let pods: Api<Pod> = match namespace {
            Some(ns) => Api::namespaced(client, &ns),
            None => Api::default_namespaced(client),
        };
        let mut result = Vec::new();

        for s in services {
            match s {
                Services::QuotingService => result.push(
                    from_params(
                        &pods,
                        "app.kubernetes.io/name=quoting-service",
                        "quoting-service",
                        Port::Name("http-api".to_string()),
                    ).await?
                ),
                Services::CentralLedger => result.push(
                    from_params(
                        &pods,
                        "app.kubernetes.io/name=centralledger-service",
                        "centralledger-service",
                        Port::Name("http-api".to_string()),
                    ).await?
                ),
                Services::MlApiAdapter => result.push(
                    from_params(
                        &pods,
                        "app.kubernetes.io/name=ml-api-adapter-service",
                        "ml-api-adapter-service",
                        Port::Number(3000),
                    ).await?
                ),
            };
        }

        Ok(result)
    }
}


async fn send_hyper_request_no_response_body(
    request_sender: &mut hyper::client::conn::SendRequest<hyper::body::Body>,
    req: hyper::Request<hyper::body::Body>
) -> Result<(http::response::Parts, hyper::body::Bytes), MojaloopCliError>
{
    let resp = request_sender.send_request(req).await
        .map_err(|e| MojaloopCliError::PortForwardConnectionError(format!("{}", e)))?;

    // Got the response okay, need to check if we have an ML API error
    let (parts, body) = resp.into_parts();

    let body_bytes = hyper::body::to_bytes(body).await
        .map_err(|e| MojaloopCliError::PortForwardResponseParseError(format!("{}", e)))?;

    if !parts.status.is_success() {
        serde_json::from_slice::<ErrorResponse>(&body_bytes)
            .map_or_else(
                |e| Err(MojaloopCliError::PortForwardResponseParseError(
                        format!("Unhandled error parsing Mojaloop API error out of response {} {}", std::str::from_utf8(&body_bytes).unwrap(), e))),
                        |ml_api_err| Err(MojaloopCliError::MojaloopApiError(ml_api_err))
            )?
    }
    Ok((parts, body_bytes))
}

async fn send_hyper_request<Resp>(
    request_sender: &mut hyper::client::conn::SendRequest<hyper::body::Body>,
    req: hyper::Request<hyper::body::Body>
// ) -> Result<(<hyper::Response<hyper::body::Body> as Trait>::Parts, Resp), MojaloopCliError>
) -> Result<(http::response::Parts, Resp), MojaloopCliError>
where
    Resp: serde::de::DeserializeOwned,
{
    let (parts, body_bytes) = send_hyper_request_no_response_body(request_sender, req).await?;
    let status = parts.status.as_u16();

    if body_bytes.len() == 0 {
        return Err(MojaloopCliError::PortForwardResponseNoBody)
    }

    let body_obj = match status {
        200..=202 => serde_json::from_slice::<Resp>(&body_bytes)
            .map_err(|e| MojaloopCliError::PortForwardResponseParseError(
                format!("Unhandled error parsing body out of response {} {}", std::str::from_utf8(&body_bytes).unwrap(), e))),
        s => Err(MojaloopCliError::PortForwardUnhandledResponse(format!("{}", s))),
    }?;

    Ok((parts, body_obj))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts: Opts = Opts::parse();

    let mut port_forwards = port_forward::get(&opts.namespace, &[
        port_forward::Services::CentralLedger,
        port_forward::Services::MlApiAdapter,
        port_forward::Services::QuotingService,
    ]).await?;
    // In reverse order than they were requested
    let quoting_service_stream = port_forwards.pop().unwrap();
    let ml_api_adapter_stream = port_forwards.pop().unwrap();
    let central_ledger_stream = port_forwards.pop().unwrap();

    let (mut quoting_service_request_sender, quoting_service_connection) = Builder::new()
        .handshake(quoting_service_stream)
        .await?;

    // spawn a task to poll the connection and drive the HTTP state
    tokio::spawn(async move {
        if let Err(e) = quoting_service_connection.await {
            eprintln!("Error in connection: {}", e);
        }
    });

    let (mut ml_api_adapter_request_sender, ml_api_adapter_connection) = Builder::new()
        .handshake(ml_api_adapter_stream)
        .await?;

    // spawn a task to poll the connection and drive the HTTP state
    tokio::spawn(async move {
        if let Err(e) = ml_api_adapter_connection.await {
            eprintln!("Error in connection: {}", e);
        }
    });

    let (mut central_ledger_request_sender, central_ledger_connection) = Builder::new()
        .handshake(central_ledger_stream)
        .await?;

    // spawn a task to poll the connection and drive the HTTP state
    tokio::spawn(async move {
        if let Err(e) = central_ledger_connection.await {
            eprintln!("Error in connection: {}", e);
        }
    });

    // TODO: collect a list of actions to take, then pass them to a function that takes those
    // actions. This will make a --dry-run option easier. It will also make a declarative format
    // (i.e. "I want this config") easier. Operations could look like this:
    // enum Operations {
    //   CreateHubMultilateralSettlementAccount({ currency: Currency }),
    //   CreateParticipantAccount({ currency: Currency }),
    // }
    // This would serialise fairly easily to something presentable to a user, and consumable by
    // code. It could get complicated if the output of one command guided subsequent commands. But
    // perhaps it's better to let the user make decisions? Alternatively, we _could_ have a tree of
    // actions and encode inputs and outputs. Getting pretty complex now, maybe not the best idea.
    // This is what a programming language is for. This tool should instead focus on simplicity.
    // The Operations and corresponding functionality could be exported to be used elsewhere.
    // let operations = match opts.subcmd {
    match opts.subcmd {
        SubCommand::Quote(quote_args) => {
            match quote_args.subcmd {
                QuoteSubCommand::Create(quote_create_args) => {
                    let post_quote = build_post_quotes(
                        quote_create_args.from,
                        quote_create_args.to,
                        quote_create_args.amount,
                        quote_create_args.currency,
                    );
                    let (quote_id, transaction_id) = if let FspiopRequestBody::PostQuotes(body) = &post_quote.body {
                        (body.quote_id, body.transaction_id)
                    } else {
                        panic!();
                    };
                    let request = fspiox_api::to_hyper_request(post_quote).unwrap();
                    send_hyper_request_no_response_body(&mut quoting_service_request_sender, request).await?;
                    println!("{{ \"quote_id\": \"{}\", \"transaction_id\": \"{}\" }}", quote_id, transaction_id);
                }
            }
        }

        SubCommand::Transfer(transfer_args) => {
            match transfer_args.subcmd {
                TransferSubCommand::Prepare(transfer_prepare_args) => {
                    match transfer_prepare_args.subcmd {
                        TransferPrepareSubCommand::New(transfer_prepare_new_args) => {
                            let transfer_prepare = build_transfer_prepare(
                                transfer_prepare_new_args.from,
                                transfer_prepare_new_args.to,
                                transfer_prepare_new_args.amount,
                                transfer_prepare_new_args.currency,
                                None,
                            );
                            let transfer_id = if let FspiopRequestBody::TransferPrepare(body) = &transfer_prepare.body {
                                body.transfer_id
                            } else {
                                panic!();
                            };
                            let request = fspiox_api::to_hyper_request(transfer_prepare).unwrap();
                            send_hyper_request_no_response_body(&mut ml_api_adapter_request_sender, request).await?;
                            println!("{}", transfer_id);
                        },
                        TransferPrepareSubCommand::FromTransaction(transfer_prepare_from_transaction_args) => {
                            // TODO: dedupe this with the above, if possible
                            let transfer_prepare = build_transfer_prepare(
                                transfer_prepare_from_transaction_args.from,
                                transfer_prepare_from_transaction_args.to,
                                transfer_prepare_from_transaction_args.amount,
                                transfer_prepare_from_transaction_args.currency,
                                Some(transfer_prepare_from_transaction_args.transfer_id),
                            );
                            let transfer_id = if let FspiopRequestBody::TransferPrepare(body) = &transfer_prepare.body {
                                body.transfer_id
                            } else {
                                panic!();
                            };
                            let request = fspiox_api::to_hyper_request(transfer_prepare).unwrap();
                            send_hyper_request_no_response_body(&mut ml_api_adapter_request_sender, request).await?;
                            println!("{}", transfer_id);
                        },
                    }
                }
            }
        }

        SubCommand::Hub(hub_args) => {
            match hub_args.subcmd {
                HubSubCommand::Accounts(hub_accs_args) => {
                    match hub_accs_args.subcmd {
                        HubAccountsSubCommand::Create(hub_accs_create_args) => {
                            async fn create_hub_account(
                                request_sender: &mut hyper::client::conn::SendRequest<hyper::body::Body>,
                                currency: Currency,
                                r#type: HubAccountType
                            ) -> Result<(), MojaloopCliError> {
                                let request = to_hyper_request(PostHubAccount {
                                    name: "Hub".to_string(),
                                    account: HubAccount {
                                        r#type,
                                        currency,
                                    }
                                }).unwrap();
                                send_hyper_request_no_response_body(request_sender, request).await?;
                                let str_hub_acc_type = match r#type {
                                    HubAccountType::HubMultilateralSettlement => "settlement",
                                    HubAccountType::HubReconciliation => "reconciliation",
                                };
                                println!("Created hub {} account: {}", str_hub_acc_type, currency);
                                Ok(())
                            }
                            match hub_accs_create_args.subcmd {
                                HubAccountsCreateSubCommand::Rec(hub_accs_create_rec_args) => {
                                    for currency in &hub_accs_create_rec_args.currencies {
                                        create_hub_account(&mut central_ledger_request_sender, *currency, HubAccountType::HubReconciliation).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::Sett(hub_accs_create_sett_args) => {
                                    for currency in &hub_accs_create_sett_args.currencies {
                                        create_hub_account(&mut central_ledger_request_sender, *currency, HubAccountType::HubMultilateralSettlement).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::All(hub_accs_create_all_args) => {
                                    for currency in &hub_accs_create_all_args.currencies {
                                        create_hub_account(&mut central_ledger_request_sender, *currency, HubAccountType::HubReconciliation).await?;
                                        create_hub_account(&mut central_ledger_request_sender, *currency, HubAccountType::HubMultilateralSettlement).await?;
                                    }
                                }
                            }
                        }
                        HubAccountsSubCommand::List => {
                            // TODO: might need to take hub name as a parameter, in order to
                            // support newer and older hub names of "hub" and "Hub"? Or just don't
                            // support old hub name? Or just try both?
                            let request = to_hyper_request(GetDfspAccounts { name: "Hub".to_string() })?;
                            let (_, accounts) = send_hyper_request::<DfspAccounts>(&mut central_ledger_request_sender, request).await?;
                            let table = accounts.iter().map(|a| vec![
                                a.ledger_account_type.cell(),
                                a.currency.cell(),
                                (if a.is_active == 1 { true } else { false }).cell(),
                                a.changed_date.cell(),
                                a.value.cell(),
                                a.reserved_value.cell(),
                            ])
                            .table()
                            .title(vec![
                                "Account type".cell(),
                                "Currency".cell(),
                                "Active".cell(),
                                "Changed date".cell(),
                                "Notification threshold".cell(),
                                "Reserved value".cell(),
                            ]);
                            print_stdout(table)?;
                        }
                    }
                }
            }
        }

        SubCommand::Participants(ps_args) => {
            match ps_args.subcmd {
                ParticipantsSubCommand::List => {
                    let request = to_hyper_request(GetParticipants {})?;
                    let (_, participants) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participants>(&mut central_ledger_request_sender, request).await?;

                    // TODO:
                    // 0. _really_ compress the output here, it's so sparse, making it quite
                    //    difficult to consume
                    // 1. additional CLI parameters to get more information about participants
                    //     -e endpoints
                    //     -f format
                    //     -a detailed account info

                    for p in participants {
                        println!(
                            "Name: {}. Active: {}. Created: {}.",
                            p.name,
                            if p.is_active == 1 { true } else { false },
                            p.created,
                        );
                        let table = p.accounts.iter().map(|a| vec![
                            a.ledger_account_type.cell(),
                            a.currency.cell(),
                            (if a.is_active == 1 { true } else { false }).cell(),
                        ])
                        .table()
                        .title(vec!["Account type".cell(), "Currency".cell(), "Active".cell()]);

                        print_stdout(table)?;
                        println!("");
                    }
                }
            }
        }

        SubCommand::Participant(p_args) => {
            match &p_args.subcmd {
                ParticipantSubCommand::Create(pc) => {
                    let request = to_hyper_request(GetParticipants {})?;
                    let (_, existing_participants) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participants>(&mut central_ledger_request_sender, request).await?;

                    match existing_participants.iter().find(|p| p.name == p_args.name) {
                        Some(existing_participant) => {
                            println!("Participant {} already exists.", existing_participant.name);
                        },
                        None => {
                            let post_participants_request = to_hyper_request(PostParticipant {
                                participant: NewParticipant {
                                    name: p_args.name.clone(),
                                    currency: pc.currency,
                                },
                            })?;
                            let (_, post_participants_result) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participant>(&mut central_ledger_request_sender, post_participants_request).await?;
                            println!("Post participants result:\n{:?}", post_participants_result);

                            let post_initial_position_and_limits_req = to_hyper_request(
                                PostInitialPositionAndLimits {
                                    name: p_args.name.clone(),
                                    initial_position_and_limits: InitialPositionAndLimits {
                                        currency: pc.currency,
                                        limit: Limit {
                                            r#type: LimitType::NetDebitCap,
                                            value: pc.ndc,
                                        },
                                        initial_position: pc.position,
                                    }
                                }
                            )?;
                            let (result, _) = send_hyper_request_no_response_body(&mut central_ledger_request_sender, post_initial_position_and_limits_req).await?;
                            println!("Post initial position and limits result:\n{:?}", result.status);

                            // TODO: 

                        },
                    }
                }
                ParticipantSubCommand::Accounts(pa) => {
                    match &pa.subcmd {
                        ParticipantAccountsSubCommand::Fund(part_acc_fund_args) => {
                            println!("part_acc_fund_args {:?}", part_acc_fund_args);
                        }
                        ParticipantAccountsSubCommand::List => {
                            let request = to_hyper_request(GetDfspAccounts { name: p_args.name })?;
                            let (_, accounts) = send_hyper_request::<DfspAccounts>(&mut central_ledger_request_sender, request).await?;
                            // TODO: table
                            println!("{:?}", accounts);
                        }
                        ParticipantAccountsSubCommand::Upsert(acc) => {
                            println!("participant account upsert {:?}", acc);
                            // 1. get participant, make an error if it doesn't exist
                            // 2. get existing accounts
                            // 3. check what was requested, compare with what we have
                        // let existing_accounts_in_switch =
                        //     send(
                        //         http_client,
                        //         host,
                        //         GetDfspAccounts { name: name.to_string() }
                        //     )
                        //     .await
                        }
                    }
                }
            }
        },

        SubCommand::Accounts(accs) => {
            match accs.subcmd {
                AccountsSubCommand::Create(a) => {
                    println!("account create {:?}", a);
                }
            }
        },
    }

    Ok(())
}
