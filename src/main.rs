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

use strum::IntoEnumIterator;
use strum_macros::Display;

use mojaloop_api::{
    common::to_hyper_request,
    central_ledger::participants::{HubAccountType, PostCallbackUrl, GetCallbackUrls, GetParticipants, Limit, LimitType, PostParticipant, InitialPositionAndLimits, GetDfspAccounts, HubAccount, PostHubAccount, DfspAccounts, PostInitialPositionAndLimits, NewParticipant, FspiopCallbackType},
    central_ledger::settlement_models,
    central_ledger::participants,
};
use fspiox_api::{
    build_post_quotes, build_transfer_prepare, FspiopRequestBody,
    common::{Amount, Currency, FspId, ErrorResponse, CorrelationId},
    transfer,
};

extern crate clap;
use clap::Clap;

use futures::{StreamExt, TryStreamExt};
use thiserror::Error;

use hyper::http::{Request, StatusCode};
use hyper::{client::conn::Builder, Body};

use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, DeleteParams, ListParams, PostParams, WatchEvent},
    Client, ResourceExt,
};

use cli_table::{print_stdout, Cell, Table};

use std::convert::TryFrom;

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
    kubeconfig: Option<std::path::PathBuf>,

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
    #[clap(alias = "p")]
    Participant(Participant),
    /// Create, read, enable, and disable accounts
    #[clap(alias = "acc")]
    Accounts(Accounts),
    /// List participants
    #[clap(alias = "ps")]
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
    #[clap(alias = "q")]
    Quote(Quote),
    /// Complex behaviours and scenarios that require a component deployed to the cluster to
    /// simulate participants.
    Voodoo(Voodoo),
    // /// Onboard a participant
    // #[clap(alias = "ob")]
    // Onboard(Onboard),
}

// #[derive(Clap)]
// struct Onboard {
//     #[clap(index = 1, required = true)]
//     name: FspId,
//     #[clap(index = 2, required = true)]
//     currency: Currency,
//     // TODO: require HTTP
//     #[clap(index = 3, required = true)]
//     hostname: url::Url,
//     #[clap(default_value = "0")]
//     ndc: u32,
//     #[clap(default_value = "0")]
//     position: Amount,
// }

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
    currency: fspiox_api::common::Currency,
    // TODO: take multiple
    #[clap(index = 4, required = true)]
    amount: Amount,
}

#[derive(Clap)]
struct Voodoo {
    // TODO: evaluate, uncomment:
    // #[clap(short,long)]
    // /// Create any participants, accounts etc. required by this command where they do not exist.
    // ///
    // /// If participants used by this command do exist, this utility will exit with an error before
    // /// taking any action. To use existing participants, if they exist, and create them if they do
    // /// not exist, combine this flag with the --hijack flag.
    // create: bool,
    // /// Disable participants and accounts created by this command.
    // #[clap(short,long)]
    // cleanup_created: bool,
    // /// Disable any participants and accounts used by this command once the command has been executed
    // ///
    // /// Disable participants and accounts used by this command. Warning: this will disable
    // /// participants and accounts that existed _before_ this command was called.
    // #[clap(short,long)]
    // cleanup: bool,
    // /// Take control of any participants specified in this command
    // ///
    // /// This will temporarily reroute all endpoints for any participants used in this command to
    // /// puppeteer. This means the entity normally configured to receive FSPIOP requests at these
    // /// endpoints will not receive them. Endpoints will be restored after the command completes.
    // #[clap(short,long)]
    // hijack: bool,
    #[clap(subcommand)]
    subcmd: VoodooSubCommand,
}

#[derive(Clap)]
enum VoodooSubCommand {
    /// Perform an end-to-end transfer, without a quote
    Transfer(PuppetTransfer),
}

#[derive(Clap)]
struct PuppetTransfer {
    payer: FspId,
    payee: FspId,
    currency: fspiox_api::common::Currency,
    amount: Amount,
    transfer_id: Option<transfer::TransferId>,
}

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
    currency: fspiox_api::common::Currency,
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
    Accounts(HubAccounts),
    /// Create settlement models
    SettlementModel(SettlementModel),
}

#[derive(Clap)]
struct HubAccounts {
    #[clap(subcommand)]
    subcmd: HubAccountsSubCommand,
}

#[derive(Clap)]
struct SettlementModel {
    #[clap(subcommand)]
    subcmd: SettlementModelSubCommand,
}

#[derive(Clap)]
enum SettlementModelSubCommand {
    Create(SettlementModelCreate),
}

#[derive(Clap)]
struct SettlementModelCreate {
    currency: fspiox_api::common::Currency,
    #[clap(short, long, default_value = "DEFERREDNET")]
    name: String,
    #[clap(short, long, parse(try_from_str), default_value = "true")]
    auto_position_reset: bool,
    #[clap(short, long, default_value = "position")]
    ledger_account_type: settlement_models::LedgerAccountType,
    #[clap(short = 't', long, default_value = "settlement")]
    settlement_account_type: settlement_models::SettlementAccountType,
    #[clap(short, long, parse(try_from_str), default_value = "true")]
    require_liquidity_check: bool,
    #[clap(short = 'd', long, default_value = "deferred")]
    settlement_delay: settlement_models::SettlementDelay,
    #[clap(short = 'g', long, default_value = "net")]
    settlement_granularity: settlement_models::SettlementGranularity,
    #[clap(short = 'i', long, default_value = "multilateral")]
    settlement_interchange: settlement_models::SettlementInterchange,
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
    #[clap(alias = "sett")]
    Settlement(HubAccountsCreateOpts),
    /// Create reconciliation accounts
    #[clap(alias = "rec")]
    Reconciliation(HubAccountsCreateOpts),
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
    // TODO: a "describe" that fetches more/better info?
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
    #[clap(alias = "acc")]
    Accounts(ParticipantAccount),
    /// Create a participant
    #[clap(alias = "add")]
    Onboard(ParticipantOnboard),
    /// Modify participant endpoints
    #[clap(alias = "ep")]
    Endpoints(ParticipantEndpoints),
    #[clap(alias = "lim")]
    /// Manage participant NDC
    Limits(ParticipantLimits),
    // TODO: there's no option here to _just_ create a participant- we should probably have one
}

#[derive(Clap)]
struct ParticipantLimits {
    #[clap(subcommand)]
    subcmd: ParticipantLimitsSubCommand,
}

#[derive(Clap)]
enum ParticipantLimitsSubCommand {
    // TODO: List(ParticipantLimitsGet),
    // TODO: is it possible to chain these, i.e. set MMD 10000 set XOF 10000 set EUR 5000 etc.?
    /// Set participant NDC
    Set(ParticipantLimitsSet),
}

#[derive(Clap)]
struct ParticipantLimitsSet {
    currency: Currency,
    value: u32,
}

#[derive(Clap)]
struct ParticipantEndpoints {
    #[clap(subcommand)]
    subcmd: ParticipantEndpointsSubCommand,
}

#[derive(Clap)]
enum ParticipantEndpointsSubCommand {
    List,
    Set(ParticipantEndpointsSet),
}

#[derive(Clap)]
struct ParticipantEndpointsSet {
    #[clap(subcommand)]
    subcmd: ParticipantEndpointsSetSubCommand,
}

#[derive(Clap)]
enum ParticipantEndpointsSetSubCommand {
    All(ParticipantEndpointsSetAll),
}

#[derive(Clap)]
struct ParticipantEndpointsSetAll {
    // TODO: require HTTP
    url: url::Url,
}

#[derive(Clap)]
struct ParticipantOnboard {
    /// The currency of the initial account to create for this participant
    #[clap(required = true)]
    currency: Currency,
    /// The host to which all FSPIOP requests destined for this participant will be delivered
    #[clap(required = true)]
    url: url::Url,
    // TODO: Accept numbers with commas, perhaps scientific notation, perhaps the 1M 1MM etc.
    // notation. Or 10K 10M 100M etc.
    /// The net debit cap for the currency account created with this command
    #[clap(default_value = "0")]
    ndc: u32,
    // TODO: Accept numbers with commas, perhaps scientific notation, perhaps the 1M 1MM etc.
    // notation. Or 10K 10M 100M etc.
    /// The initial position of the currency account created with this command
    #[clap(default_value = "0")]
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
    // TODO: only prints active accounts, at present- should have a --inactive flag? Or just print
    // is_active status?
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

#[derive(Debug, Display, Clone)]
pub enum Port {
    Name(String),
    Number(i32),
}

// TODO: crate::Result (i.e. a result type for this crate, that uses this error type as its error
// type). Probably just call this type "Error"?
#[derive(Error, Debug)]
pub enum MojaloopCliError {
    #[error("Couldn't find pod with label: {0}")]
    PodNotFound(String),
    #[error("Unexpected manifest implementation for pod: {0}")]
    UnexpectedPodImplementation(String),
    #[error("Service container {0} not found in pod: {1}")]
    ServiceContainerNotFound(String, String),
    #[error("Couldn't find port {0} on service container: {1}")]
    ServicePortNotFound(Port, String),
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
    #[error("Couldn't load kubeconfig file: {0}")]
    UnableToLoadKubeconfig(String),
    // TODO: tell the user what command to execute to create such an account?
    #[error("Participant {0} does not have {1} settlement account")]
    ParticipantMissingCurrencyAccount(FspId, Currency),
}

async fn get_pods(
    kubeconfig: &Option<std::path::PathBuf>,
    namespace: &Option<String>,
) -> Result<Api<Pod>, MojaloopCliError> {
    let client = match kubeconfig {
        Some(path) => {
            let custom_config = kube::config::Kubeconfig::read_from(path.as_path())
                .map_err(|e| MojaloopCliError::UnableToLoadKubeconfig(e.to_string()))?;
            // TODO: expose some of this to the user?
            let options = kube::config::KubeConfigOptions {
                context: None,
                cluster: None,
                user: None,
            };
            let config = kube::Config::from_custom_kubeconfig(custom_config, &options).await
                .map_err(|e| MojaloopCliError::UnableToLoadKubeconfig(e.to_string()))?;
            Client::try_from(config)
                .map_err(|e| MojaloopCliError::UnableToLoadKubeconfig(e.to_string()))?
        },
        None => Client::try_default().await
            .map_err(|e| MojaloopCliError::UnableToLoadKubeconfig(e.to_string()))?
    };
    Ok(
        match namespace {
            Some(ns) => Api::namespaced(client, &ns),
            None => Api::default_namespaced(client),
        }
    )
}

mod port_forward {
    use kube::api::{Api, ListParams};
    use k8s_openapi::api::core::v1::Pod;
    use crate::MojaloopCliError;
    use std::convert::TryInto;

    // TODO: the presence of Port here probably suggests that many of the top-level errors here
    // should be in the port_forward module. Or the port_forward module should be flattened into
    // the file.
    use crate::Port;

    // TODO: somehow, when establishing port-forward fails because the pod is still coming up, this
    // doesn't cause the application to fail.
    pub async fn from_params(
        pods: &Api<Pod>,
        label: &str,
        container_name: &str,
        port: Port,
    ) -> anyhow::Result<(impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin)> {
        let lp = ListParams::default().labels(&label);
        let pod = pods
            .list(&lp).await.map_err(|e| MojaloopCliError::ClusterConnectionError(e))?
            .items.get(0).ok_or(MojaloopCliError::PodNotFound(label.to_string()))?.clone();
        let pod_name = pod.metadata.name.clone().unwrap();
        let pod_port = pod
            .spec
            .ok_or(MojaloopCliError::UnexpectedPodImplementation(pod_name.clone()))?
            .containers.iter().find(|c| c.name == container_name)
            .ok_or(MojaloopCliError::ServiceContainerNotFound(container_name.to_string(), pod_name.clone()))?
            .ports.as_ref().ok_or(MojaloopCliError::ServicePortNotFound(port.clone(), container_name.to_string()))?
            .iter()
            .find(|p| {
                match &port {
                    Port::Name(port_name) => p.name.as_ref().map_or(false, |name| name == port_name),
                    Port::Number(port_num) => p.container_port == *port_num,
                }
            })
            .ok_or(MojaloopCliError::ServicePortNotFound(port, container_name.to_string()))?.clone();
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

    pub async fn get(
        pods: &Api<Pod>,
        services: &[Services]
    ) ->
        anyhow::Result<Vec<(impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin)>>
    {
        let mut result = Vec::new();

        for s in services {
            match s {
                Services::QuotingService => result.push(
                    from_params(
                        pods,
                        "app.kubernetes.io/name=quoting-service",
                        "quoting-service",
                        Port::Name("http-api".to_string()),
                    ).await?
                ),
                Services::CentralLedger => result.push(
                    from_params(
                        pods,
                        "app.kubernetes.io/name=centralledger-service",
                        "centralledger-service",
                        Port::Name("http-api".to_string()),
                    ).await?
                ),
                Services::MlApiAdapter => result.push(
                    from_params(
                        pods,
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

    let pods = get_pods(
        &opts.kubeconfig,
        &opts.namespace,
    ).await?;

    let mut port_forwards = port_forward::get(
        &pods,
        &[
            port_forward::Services::CentralLedger,
            port_forward::Services::MlApiAdapter,
            port_forward::Services::QuotingService,
        ]
    ).await?;
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

    async fn set_participant_endpoints(
        participant_name: &String,
        url: &String,
        request_sender: &mut hyper::client::conn::SendRequest<hyper::body::Body>,
    ) -> anyhow::Result<()> {
        // TODO: strip trailing slash
        for callback_type in FspiopCallbackType::iter() {
            let request = to_hyper_request(PostCallbackUrl {
                name: participant_name.clone(),
                callback_type,
                // TODO: strip trailing slash
                hostname: url.clone(),
            })?;
            let (result, _) = send_hyper_request_no_response_body(request_sender, request).await?;
            // TODO: url.clone() is just the hostname the user
                // provided, not the actual endpoint template. This could
                // be confusing. We should show the whole endpoint
                // template.
            println!("Updated {:?} endpoint to {}. Response {}.", callback_type, url.clone(), result.status);
        }
        Ok(())
    }

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
                HubSubCommand::SettlementModel(hub_settlement_model_args) => {
                    match hub_settlement_model_args.subcmd {
                        SettlementModelSubCommand::Create(hub_settlement_model_create_args) => {
                            let request = to_hyper_request(settlement_models::PostSettlementModel {
                                settlement_model: settlement_models::SettlementModel {
                                    auto_position_reset: hub_settlement_model_create_args.auto_position_reset,
                                    ledger_account_type: hub_settlement_model_create_args.ledger_account_type,
                                    settlement_account_type: hub_settlement_model_create_args.settlement_account_type,
                                    name: hub_settlement_model_create_args.name.clone(),
                                    require_liquidity_check: hub_settlement_model_create_args.require_liquidity_check,
                                    settlement_delay: hub_settlement_model_create_args.settlement_delay,
                                    settlement_granularity: hub_settlement_model_create_args.settlement_granularity,
                                    settlement_interchange: hub_settlement_model_create_args.settlement_interchange,
                                    currency: hub_settlement_model_create_args.currency,
                                }
                            }).unwrap();
                            send_hyper_request_no_response_body(&mut central_ledger_request_sender, request).await?;
                            println!("Created settlement model: {}", hub_settlement_model_create_args.name);
                            // Ok(())
                        }
                    }
                }
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
                                HubAccountsCreateSubCommand::Reconciliation(hub_accs_create_rec_args) => {
                                    for currency in &hub_accs_create_rec_args.currencies {
                                        create_hub_account(&mut central_ledger_request_sender, *currency, HubAccountType::HubReconciliation).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::Settlement(hub_accs_create_sett_args) => {
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
                ParticipantSubCommand::Limits(participant_limits_args) => {
                    match &participant_limits_args.subcmd {
                        ParticipantLimitsSubCommand::Set(participant_limits_set_args) => {
                            let request = to_hyper_request(participants::PutParticipantLimit {
                                name: p_args.name.clone(),
                                limit: participants::NewParticipantLimit {
                                    currency: participant_limits_set_args.currency,
                                    limit: participants::ParticipantLimit {
                                        value: participant_limits_set_args.value,
                                        r#type: participants::LimitType::NetDebitCap,
                                        alarm_percentage: 10, // TODO: expose this to the user?
                                    }
                                }
                            })?;
                            let (result, _) = send_hyper_request_no_response_body(
                                &mut central_ledger_request_sender,
                                request,
                            ).await?;
                            println!(
                                "Update {} {} limit to {} result:\n{:?}",
                                p_args.name,
                                participant_limits_set_args.currency,
                                participant_limits_set_args.value,
                                result.status
                            );
                        }
                    }
                }
                ParticipantSubCommand::Endpoints(participant_endpoints_args) => {
                    match &participant_endpoints_args.subcmd {
                        ParticipantEndpointsSubCommand::List => {
                            let request = to_hyper_request(GetCallbackUrls {
                                name: p_args.name.clone(),
                            })?;
                            let (_, endpoints) = send_hyper_request::<mojaloop_api::central_ledger::participants::CallbackUrls>(&mut central_ledger_request_sender, request).await?;
                            // TODO: table
                            for ep in endpoints.iter() {
                                println!("{} {}", ep.r#type, ep.value);
                            }
                        },
                        ParticipantEndpointsSubCommand::Set(participant_endpoints_set_args) => {
                            match &participant_endpoints_set_args.subcmd {
                                ParticipantEndpointsSetSubCommand::All(participant_endpoints_set_all_args) => {
                                    set_participant_endpoints(
                                        &p_args.name,
                                        &participant_endpoints_set_all_args.url.to_string(),
                                        &mut central_ledger_request_sender,
                                    ).await?;
                                }
                            }
                        },
                    }
                }

                ParticipantSubCommand::Onboard(participant_create_args) => {
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
                                    currency: participant_create_args.currency,
                                },
                            })?;
                            let (_, post_participants_result) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participant>(&mut central_ledger_request_sender, post_participants_request).await?;
                            println!("Post participants result:\n{:?}", post_participants_result);

                            let post_initial_position_and_limits_req = to_hyper_request(
                                PostInitialPositionAndLimits {
                                    name: p_args.name.clone(),
                                    initial_position_and_limits: InitialPositionAndLimits {
                                        currency: participant_create_args.currency,
                                        limit: Limit {
                                            r#type: LimitType::NetDebitCap,
                                            value: participant_create_args.ndc,
                                        },
                                        initial_position: participant_create_args.position,
                                    }
                                }
                            )?;
                            let (result, _) = send_hyper_request_no_response_body(&mut central_ledger_request_sender, post_initial_position_and_limits_req).await?;
                            println!("Post initial position and limits result:\n{:?}", result.status);

                            set_participant_endpoints(
                                &p_args.name,
                                &participant_create_args.url.to_string(),
                                &mut central_ledger_request_sender,
                            ).await?;
                        },
                    }
                }

                ParticipantSubCommand::Accounts(pa) => {
                    match &pa.subcmd {
                        ParticipantAccountsSubCommand::Fund(part_acc_fund_args) => {
                            let get_accounts = to_hyper_request(participants::GetDfspAccounts{
                                name: p_args.name.clone(),
                            }).unwrap();
                            let (_, accounts) = send_hyper_request::<DfspAccounts>(
                                &mut central_ledger_request_sender, get_accounts).await?;
                            // TODO: provide helpful error messages.
                            let account = accounts
                                .iter()
                                .find(|acc|
                                    acc.ledger_account_type == participants::AnyAccountType::Settlement &&
                                    acc.currency == part_acc_fund_args.currency,
                                )
                                .map(Ok)
                                .unwrap_or(Err(MojaloopCliError::ParticipantMissingCurrencyAccount(
                                            p_args.name.clone(), part_acc_fund_args.currency)))?;
                            match &part_acc_fund_args.subcmd {
                                ParticipantAccountFundSubCommand::In(part_acc_fund_in_args) => { println!("Not yet implemented") },
                                ParticipantAccountFundSubCommand::Out(part_acc_fund_out_args) => { println!("Not yet implemented") },
                                ParticipantAccountFundSubCommand::Num(part_acc_fund_num_args) => {
                                    let action = if part_acc_fund_num_args.amount > Amount::ZERO {
                                        participants::ParticipantFundsInOutAction::RecordFundsIn
                                    } else {
                                        participants::ParticipantFundsInOutAction::RecordFundsOutPrepareReserve
                                    };
                                    let funds_request = to_hyper_request(
                                        participants::PostParticipantSettlementFunds {
                                            name: p_args.name,
                                            account_id: account.id,
                                            funds: participants::ParticipantFundsInOut {
                                                transfer_id: fspiox_api::common::CorrelationId::new(),
                                                action,
                                                amount: fspiox_api::common::Money {
                                                    currency: part_acc_fund_args.currency,
                                                    amount: part_acc_fund_num_args.amount.abs()
                                                },
                                                reason: "Voodoo".to_string(),
                                                external_reference: "Voodoo".to_string(),
                                            }
                                        }
                                    )?;
                                    let (result, _) = send_hyper_request_no_response_body(
                                        &mut central_ledger_request_sender,
                                        funds_request,
                                    ).await?;
                                    println!("Funds in result:\n{:?}", result.status);
                                }
                            }
                        }
                        ParticipantAccountsSubCommand::List => {
                            let request = to_hyper_request(GetDfspAccounts { name: p_args.name })?;
                            let (_, accounts) = send_hyper_request::<DfspAccounts>(&mut central_ledger_request_sender, request).await?;
                            // TODO: table
                            // println!("{:?}", accounts);
                            for acc in accounts {
                                if acc.is_active == 1 {
                                    println!("{} {} {}", acc.currency, acc.ledger_account_type, acc.value);
                                }
                            }
                        }
                        ParticipantAccountsSubCommand::Upsert(acc) => {
                            println!("Not yet implemented");
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

        SubCommand::Voodoo(voodoo_args) => {
            use voodoo_doll::protocol::*;
            use futures_util::StreamExt;
            use tokio_tungstenite::{client_async, tungstenite::protocol::Message};
            use futures::SinkExt;

            let p: Pod = voodoo_doll::pod().unwrap();

            // TODO: here we fail if the pod exists or is being created/deleted- need to handle
            // this better.
            pods.create(
                &kube::api::PostParams::default(),
                &p,
            ).await?;

            // Wait until the pod is running
            let pod_name = p.metadata.name.unwrap();
            let lp = ListParams::default()
                .fields(format!("metadata.name={}", &pod_name).as_str())
                .timeout(30);
            let mut stream = pods.watch(&lp, "0").await?.boxed();
            while let Some(status) = stream.try_next().await? {
                match status {
                    WatchEvent::Added(o) => {
                        println!("Added {}", o.name());
                    }
                    WatchEvent::Modified(o) => {
                        let s = o.status.as_ref().expect("status exists on pod");
                        if s.phase.clone().unwrap_or_default() == "Running" {
                            break;
                        }
                    }
                    _ => {}
                }
            }

            let pod_label_key = "app.kubernetes.io/name";
            let pod_label = format!(
                "{}={}",
                pod_label_key,
                p.metadata.labels.unwrap().get(pod_label_key).unwrap(),
            );

            let voodoo_doll_stream = port_forward::from_params(
                &pods,
                // TODO: we sort of can't really know how the pod is going to be identified,
                // therefore this information should be exposed by the voodoo-doll lib, one way or
                // another. Perhaps voodoo-doll lib should have a function that accepts a pod list
                // (our &pods above) and returns the correct pod, if it is present. And creates it,
                // if not?
                &pod_label,
                &p.spec.unwrap().containers[0].name,
                Port::Number(3030),
            ).await?;

            // TODO: we sort of can't really know what endpoint to call, therefore this information
            // should be exposed by the voodoo-doll lib, one way or another.
            // let uri = "/voodoo".parse::<http::Uri>().unwrap();
            let (ws_stream, _) = client_async("ws://host.ignored/voodoo", voodoo_doll_stream).await?;

            let (mut voodoo_write, mut voodoo_read) = ws_stream.split();

            match voodoo_args.subcmd {
                VoodooSubCommand::Transfer(voodoo_transfer_args) => {
                    let transfer_id = voodoo_transfer_args.transfer_id.unwrap_or(
                        transfer::TransferId(fspiox_api::common::CorrelationId::new()));
                    voodoo_write.send(
                        Message::Text(
                            serde_json::to_string(
                                &ClientMessage::Transfer(
                                    TransferMessage {
                                        msg_sender: voodoo_transfer_args.payer,
                                        msg_recipient: voodoo_transfer_args.payee,
                                        currency: voodoo_transfer_args.currency,
                                        amount: voodoo_transfer_args.amount,
                                        transfer_id,
                                    }
                                )
                            )?
                        )
                    ).await?;

                    while let Some(msg) = voodoo_read.next().await {
                        let msg = msg?;
                        match msg {
                            Message::Text(s) => {
                                let response_msg: ServerMessage =
                                    serde_json::from_str(&s)?;
                                match response_msg {
                                    ServerMessage::TransferComplete(tc) => {
                                        if tc.id == transfer_id {
                                            println!("Transfer complete. ID: {}", transfer_id);
                                            break;
                                        }
                                    }
                                    ServerMessage::TransferError(te) => {
                                        if te.id == transfer_id {
                                            println!("Transfer error. Error: {:?}", s);
                                            break;
                                        }
                                    }
                                }
                            }
                            _ => {
                                println!("Incoming non-text:");
                                println!("{}", msg);
                            }
                        }
                    }
                }
            }

            // Cleanup
            voodoo_write.close().await?;
            // voodoo_read.close();
            pods.delete(&pod_name, &DeleteParams::default()).await?;

            // TODO: check for an existing voodoo doll in the cluster
            //
            // 1. Create a voodoo doll in the cluster (the lib should export a pod manifest that
            //    can be used to create it- or maybe one better, a function that takes a k8s client
            //    and returns a created pod?). The voodoo doll needs to have some manner of health
            //    endpoint, and the manifest needs to use that to check it's healthy, so that we
            //    can use k8s functionality to be confident it's ready. Moreover, if it is
            //    responsible for itself, then users of the library need not worry about
            //    versioning, etc. (So TODO: what will voodoo doll do about versioning....?).
            //
            //    An example of creating a pod can be found here:
            //    https://github.com/kazk/kube-rs/pull/4/files
            //
            // 2. open a websocket to the voodoo doll
            //
            // 3. send our messsage
            //
            // 4. wait for a reply
            //
            // 5. kill the voodoo doll
            //
            // TODO: can the voodoo doll have some sort of self-destruct feature, whereby after
            // some duration with no connections it will kill itself? This way, we can leave it in
            // place. Leaving it in place is advantageous, because when the pod is deleted, it
            // takes some time to go away, so if we want to perform some more voodoo (issue another
            // command to it), we will have to wait until it's deleted, then recreate it. This will
            // quickly become annoying.
            // Some mechanisms by which this could be achieved:
            // - voodoo-doll could be run as a job with no history and a generatename
            //   - https://serverfault.com/a/868826
            //   - https://stackoverflow.com/questions/41385403/how-to-automatically-remove-completed-kubernetes-jobs-created-by-a-cronjob
            //   - https://stackoverflow.com/a/54635208
            // - voodoo-doll could have a service account provided, and it could just remove itself
            //   when its time runs out
            // - voodoo-doll could be a Deployment, and could be scaled up and down on-demand,
            //   going down to zero when we don't want it around
        }
    }

    Ok(())
}
