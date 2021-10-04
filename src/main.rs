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
    central_ledger::participants,
    central_ledger::participants::{
        AnyAccountType,
        FspiopCallbackType,
        GetCallbackUrls,
        GetDfspAccounts,
        GetParticipants,
        HubAccount,
        HubAccountType,
        InitialPositionAndLimits,
        Limit,
        LimitType,
        NewParticipant,
        PostCallbackUrl,
        PostHubAccount,
        PostInitialPositionAndLimits,
        PostParticipant,
        PutParticipantAccount,
    },
    central_ledger::settlement_models,
    settlement::{settlement_windows, settlement},
};
use fspiox_api::{
    FspiopRequestBody, Amount, Currency, FspId, ErrorResponse, CorrelationId, transfer, quote,
};

extern crate clap;
use clap::Clap;

use thiserror::Error;

use k8s_openapi::api::core::v1::Pod;
use kube::{api::Api, Client};

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
    /// Create and manage settlements and settlement windows
    Settlement(Settlement),
    /// Complex behaviours and scenarios that require a component deployed to the cluster to
    /// simulate participants.
    Voodoo(Voodoo),
    // /// Onboard a participant
    // #[clap(alias = "ob")]
    // Onboard(Onboard),
}

#[derive(Clap)]
struct Settlement {
    #[clap(subcommand)]
    subcmd: SettlementSubCommand,
}

#[derive(Clap)]
enum SettlementSubCommand {
    /// Create a settlement from existing settlement windows
    #[clap(alias = "new")]
    Create(SettlementCreate),
    /// Settlement window commands
    #[clap(alias = "win", alias = "windows")]
    Window(SettlementWindow),
    // TODO: settlement model subcommand
}

#[derive(Clap)]
struct SettlementWindow {
    #[clap(subcommand)]
    subcmd: SettlementWindowSubCommand,
}

#[derive(Clap)]
enum SettlementWindowSubCommand {
    /// Close a settlement window by ID
    Close(CloseSettlementWindow),
    /// Show a settlement window by ID
    Get(GetSettlementWindow),
    // TODO: create a "list" subcommand that lists all settlement windows? Or just have that as the
    // default "no filters" option to the filter subcommand.
    /// Filter settlement windows
    Filter(FilterSettlementWindows),
}

#[derive(Clap)]
struct GetSettlementWindow {
    id: settlement_windows::SettlementWindowId,
}

#[derive(Clap)]
struct FilterSettlementWindows {
    // TODO: should support multiple states
    /// The settlement window state
    #[clap(default_value = "OPEN")]
    state: settlement_windows::SettlementWindowState,
    // TODO: should support other filter options
}

#[derive(Clap)]
struct CloseSettlementWindow {
    #[clap(default_value = "Mojaloop CLI request", long, short)]
    reason: String,
    #[clap(index = 1)]
    id: settlement_windows::SettlementWindowId,
}

#[derive(Clap)]
struct SettlementCreate {
    #[clap(short, long, default_value = "Mojaloop CLI request")]
    reason: String,
    #[clap(index = 1, required = true)]
    settlement_model: String,
    #[clap(index = 2, required = true, multiple = true)]
    settlement_window_ids: Vec<settlement_windows::SettlementWindowId>,
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
    currency: fspiox_api::Currency,
    // TODO: take multiple
    #[clap(index = 4, required = true)]
    amount: Amount,
}

#[derive(Clap)]
struct Voodoo {
    // TODO: a command here that just hijacks a given participants endpoints. This way, we can
    // deploy a voodoo-doll instance, hijack a participant or two, and send transfers at our
    // leisure. The reason we can't do this at present is because some entity needs to receive the
    // transfers in order for them to not be rejected by the switch. (Maybe set up some feature to
    // control transfer timeouts also).
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
    /// Deploy the in-cluster component for the voodoo subcommand
    #[clap(long)]
    deploy: bool,
    /// Destroy the in-cluster component after running this command
    #[clap(long)]
    destroy: bool,
    #[clap(subcommand)]
    subcmd: VoodooSubCommand,
}

#[derive(Clap, PartialEq, Eq, Clone)]
enum VoodooSubCommand {
    // TODO: version argument to Deploy?
    /// Deploy the in-cluster component of this tool
    Deploy,
    /// Destroy the in-cluster component of this tool
    Destroy,
    /// Perform an end-to-end transfer, without a quote
    Transfer(PuppetTransfer),
}

#[derive(Clap, PartialEq, Eq, Clone)]
struct PuppetTransfer {
    payer: FspId,
    payee: FspId,
    currency: fspiox_api::Currency,
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
    currency: fspiox_api::Currency,
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
    currency: fspiox_api::Currency,
    #[clap(short, long, default_value = "DEFERREDNET")]
    name: mojaloop_api::central_ledger::settlement_models::SettlementModelName,
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
    #[clap(alias = "ls", alias = "l")]
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
    #[clap(alias = "add", alias = "ob")]
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
    #[clap(alias = "list", alias = "view")]
    /// Get participant NDC
    Get,
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
    /// Disable participant account
    Disable(ParticipantAccountDisable),
    /// Disable participant account
    Enable(ParticipantAccountEnable),
}

#[derive(Clap, Debug)]
struct ParticipantAccountDisable {
    // TODO: accept multiple currencies
    #[clap(index = 1, required = true, multiple = true)]
    currency: Vec<Currency>,
}

#[derive(Clap, Debug)]
struct ParticipantAccountEnable {
    // TODO: accept multiple currencies
    #[clap(index = 1, required = true, multiple = true)]
    currency: Vec<Currency>,
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
    #[error("Failed to connect to voodoo doll: {0}")]
    VoodooDollConnectionError(String),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use mojaloop_api::clients::FspiopClient;
    let opts: Opts = Opts::parse();

    let pods = fspiox_api::clients::k8s::get_pods(
        &opts.kubeconfig,
        &opts.namespace,
    ).await?;

    async fn set_participant_endpoints(
        participant_name: &FspId,
        url: &String,
        client: &mut mojaloop_api::clients::central_ledger::Client,
    ) -> anyhow::Result<()> {
        // TODO: strip trailing slash
        for callback_type in FspiopCallbackType::iter() {
            let request = PostCallbackUrl {
                name: participant_name.clone(),
                callback_type,
                // TODO: strip trailing slash
                hostname: url.clone(),
            };
            let result = client.send(request).await?;
            // TODO: url.clone() is just the hostname the user provided, not the actual endpoint
            // template. This could be confusing. We should show the whole endpoint template.
            println!("Updated {:?} endpoint to {}", callback_type, url.clone());
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
        SubCommand::Settlement(settlement_args) => {
            // TODO: if we implement pools in fspiox_api with a minimum connection count of zero,
            // we could "get" all clients at once, and lazily connect to them. This would make
            // getting clients much more elegant.
            let mut ml_settlement = mojaloop_api::clients::settlement::Client::from_k8s_params(
                &opts.kubeconfig,
                &opts.namespace,
                Some(pods),
            ).await?;
            match settlement_args.subcmd {
                SettlementSubCommand::Window(window_args) => {
                    match window_args.subcmd {
                        SettlementWindowSubCommand::Get(get_window_args) => {
                            let request = settlement_windows::GetSettlementWindow {
                                id: get_window_args.id
                            };

                            let window = ml_settlement.send(request).await?.des().await?;
                            println!("{:?}", window);
                        }

                        SettlementWindowSubCommand::Filter(filter_window_args) => {
                            let request = settlement_windows::GetSettlementWindows {
                                state: Some(filter_window_args.state),
                                currency: None,
                                from_date_time: None,
                                participant_id: None,
                                to_date_time: None,
                            };

                            let windows = ml_settlement.send(request).await?;

                            // TODO: table
                            println!("{:?}", windows);
                        }

                        SettlementWindowSubCommand::Close(close_window_args) => {
                            let request = settlement_windows::CloseSettlementWindow {
                                id: close_window_args.id,
                                payload: settlement_windows::SettlementWindowClosurePayload {
                                    reason: close_window_args.reason,
                                    state: settlement_windows::SettlementWindowCloseState::Closed,
                                }
                            };

                            ml_settlement.send(request).await?;

                            println!("Closed window: {}", close_window_args.id);
                        }
                    }
                }

                SettlementSubCommand::Create(create_settlement_args) => {
                    let request = settlement::PostSettlement {
                        new_settlement: settlement::NewSettlement {
                            reason: create_settlement_args.reason,
                            settlement_model: create_settlement_args.settlement_model,
                            settlement_windows: create_settlement_args.settlement_window_ids
                                .iter()
                                .map(|id| settlement::WindowParametersNewSettlement { id: *id })
                                .collect(),
                        }
                    };
                    let new_settlement = ml_settlement.send(request).await?.des().await?;

                    // TODO: handle this response:
                    // {
                    //   "errorInformation": {
                    //     "errorCode": "3100",
                    //     "errorDescription": "Generic validation error - Settlement model not found"
                    //   }
                    // }
                    // by getting available settlement models and listing them for the user

                    // TODO: pretty-print
                    println!("Created settlement ID: {:?}. Result: {:?}", new_settlement.id, new_settlement);
                }
            }
        }

        SubCommand::Quote(quote_args) => {
            let mut ml_quote = mojaloop_api::clients::quote::Client::from_k8s_params(
                &opts.kubeconfig,
                &opts.namespace,
                Some(pods),
            ).await?;
            match quote_args.subcmd {
                QuoteSubCommand::Create(quote_create_args) => {
                    let post_quote = quote::QuoteRequest::new(
                        quote_create_args.from,
                        quote_create_args.to,
                        quote_create_args.amount,
                        quote_create_args.currency,
                    );

                    // TODO: what is this weird pattern? Is it necessary?
                    let (quote_id, transaction_id) = if let FspiopRequestBody::PostQuotes(body) = &post_quote.0.body {
                        (body.quote_id, body.transaction_id)
                    } else {
                        panic!();
                    };

                    ml_quote.send(post_quote).await?;
                    println!("{{ \"quote_id\": \"{}\", \"transaction_id\": \"{}\" }}", quote_id, transaction_id);
                }
            }
        }

        SubCommand::Transfer(transfer_args) => {
            let mut ml_transfer = mojaloop_api::clients::transfer::Client::from_k8s_params(
                &opts.kubeconfig,
                &opts.namespace,
                Some(pods),
            ).await?;
            match transfer_args.subcmd {
                TransferSubCommand::Prepare(transfer_prepare_args) => {
                    match transfer_prepare_args.subcmd {
                        TransferPrepareSubCommand::New(transfer_prepare_new_args) => {
                            let transfer_prepare = transfer::TransferPrepareRequest::new(
                                transfer_prepare_new_args.from,
                                transfer_prepare_new_args.to,
                                transfer_prepare_new_args.amount,
                                transfer_prepare_new_args.currency,
                                None,
                            );

                            // TODO: what is this weird pattern? Is it necessary?
                            let transfer_id = if let FspiopRequestBody::TransferPrepare(body) = &transfer_prepare.0.body {
                                body.transfer_id
                            } else {
                                panic!();
                            };

                            ml_transfer.send(transfer_prepare).await?;

                            println!("{}", transfer_id);
                        },

                        TransferPrepareSubCommand::FromTransaction(transfer_prepare_from_transaction_args) => {
                            // TODO: dedupe this with the above, if possible
                            let transfer_prepare = transfer::TransferPrepareRequest::new(
                                transfer_prepare_from_transaction_args.from,
                                transfer_prepare_from_transaction_args.to,
                                transfer_prepare_from_transaction_args.amount,
                                transfer_prepare_from_transaction_args.currency,
                                Some(transfer_prepare_from_transaction_args.transfer_id),
                            );

                            // TODO: what is this weird pattern? Is it necessary?
                            let transfer_id = if let FspiopRequestBody::TransferPrepare(body) = &transfer_prepare.0.body {
                                body.transfer_id
                            } else {
                                panic!();
                            };

                            ml_transfer.send(transfer_prepare).await?;

                            println!("{}", transfer_id);
                        },
                    }
                }
            }
        }

        SubCommand::Hub(hub_args) => {
            let mut ml_central_ledger = mojaloop_api::clients::central_ledger::Client::from_k8s_params(
                &opts.kubeconfig,
                &opts.namespace,
                Some(pods),
            ).await?;
            match hub_args.subcmd {
                HubSubCommand::SettlementModel(hub_settlement_model_args) => {
                    match hub_settlement_model_args.subcmd {
                        SettlementModelSubCommand::Create(hub_settlement_model_create_args) => {
                            let request = settlement_models::PostSettlementModel {
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
                            };
                            ml_central_ledger.send(request).await?;
                            println!("Created settlement model: {}", hub_settlement_model_create_args.name);
                            // Ok(())
                        }
                    }
                }

                HubSubCommand::Accounts(hub_accs_args) => {
                    match hub_accs_args.subcmd {
                        HubAccountsSubCommand::Create(hub_accs_create_args) => {
                            async fn create_hub_account(
                                client: &mut mojaloop_api::clients::central_ledger::Client,
                                currency: Currency,
                                r#type: HubAccountType
                            ) -> fspiox_api::clients::Result<()> {
                                let request = PostHubAccount {
                                    // TODO: parametrise hub name?
                                    name: FspId::from("Hub").unwrap(),
                                    account: HubAccount {
                                        r#type,
                                        currency,
                                    }
                                };

                                client.send(request).await?;

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
                                        create_hub_account(&mut ml_central_ledger, *currency, HubAccountType::HubReconciliation).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::Settlement(hub_accs_create_sett_args) => {
                                    for currency in &hub_accs_create_sett_args.currencies {
                                        create_hub_account(&mut ml_central_ledger, *currency, HubAccountType::HubMultilateralSettlement).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::All(hub_accs_create_all_args) => {
                                    for currency in &hub_accs_create_all_args.currencies {
                                        create_hub_account(&mut ml_central_ledger, *currency, HubAccountType::HubReconciliation).await?;
                                        create_hub_account(&mut ml_central_ledger, *currency, HubAccountType::HubMultilateralSettlement).await?;
                                    }
                                }
                            }
                        }
                        HubAccountsSubCommand::List => {
                            // TODO: might need to take hub name as a parameter, in order to
                            // support newer and older hub names of "hub" and "Hub"? Or just don't
                            // support old hub name? Or just try both?
                            let request = GetDfspAccounts { name: FspId::from("Hub").unwrap() };
                            let accounts = ml_central_ledger.send(request).await?.des().await?;
                            let table = accounts.iter()
                                .map(|a| vec![
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
            let mut ml_central_ledger = mojaloop_api::clients::central_ledger::Client::from_k8s_params(
                &opts.kubeconfig,
                &opts.namespace,
                Some(pods),
            ).await?;
            match ps_args.subcmd {
                ParticipantsSubCommand::List => {
                    let request = GetParticipants {};
                    let participants = ml_central_ledger.send(request).await?.des().await?;
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
            let mut ml_central_ledger = mojaloop_api::clients::central_ledger::Client::from_k8s_params(
                &opts.kubeconfig,
                &opts.namespace,
                Some(pods),
            ).await?;
            match &p_args.subcmd {
                ParticipantSubCommand::Limits(participant_limits_args) => {
                    match &participant_limits_args.subcmd {
                        ParticipantLimitsSubCommand::Get => {
                            let request = participants::GetParticipantLimits {
                                name: p_args.name.clone(),
                            };

                            let limits = ml_central_ledger.send(request).await?.des().await?;
                            let table = limits.iter()
                                .map(|l| vec![
                                    l.currency.cell(),
                                    l.limit.r#type.cell(),
                                    l.limit.value.cell(),
                                ])
                                .table()
                                .title(vec![
                                    "Currency".cell(),
                                    "Type".cell(),
                                    "Value".cell(),
                                ]);
                            print_stdout(table)?;
                        }

                        ParticipantLimitsSubCommand::Set(participant_limits_set_args) => {
                            let request = participants::PutParticipantLimit {
                                name: p_args.name.clone(),
                                limit: participants::NewParticipantLimit {
                                    currency: participant_limits_set_args.currency,
                                    limit: participants::ParticipantLimit {
                                        value: participant_limits_set_args.value,
                                        r#type: participants::LimitType::NetDebitCap,
                                        alarm_percentage: 10, // TODO: expose this to the user?
                                    }
                                }
                            };
                            match ml_central_ledger.send(request).await {
                                Err(e) => {
                                    println!(
                                        "Failed to update {} {} limit to {}: {:?}\n",
                                        p_args.name,
                                        participant_limits_set_args.currency,
                                        participant_limits_set_args.value,
                                        e,
                                    );
                                }
                                _ => {
                                    println!(
                                        "Updated {} {} limit to {}\n",
                                        p_args.name,
                                        participant_limits_set_args.currency,
                                        participant_limits_set_args.value,
                                    );
                                }
                            }
                        }
                    }
                }
                ParticipantSubCommand::Endpoints(participant_endpoints_args) => {
                    match &participant_endpoints_args.subcmd {
                        ParticipantEndpointsSubCommand::List => {
                            let request = GetCallbackUrls {
                                name: p_args.name.clone(),
                            };
                            let endpoints = ml_central_ledger.send(request).await?.des().await?;
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
                                        &mut ml_central_ledger,
                                    ).await?;
                                }
                            }
                        },
                    }
                }

                ParticipantSubCommand::Onboard(participant_create_args) => {
                    let request = GetParticipants {};
                    let existing_participants = ml_central_ledger.send(request).await?.des().await?;

                    match existing_participants.iter().find(|p| p.name == p_args.name) {
                        Some(existing_participant) => {
                            println!("Participant {} already exists.", existing_participant.name);
                        },
                        None => {
                            let post_participants_request = PostParticipant {
                                participant: NewParticipant {
                                    name: p_args.name.clone(),
                                    currency: participant_create_args.currency,
                                },
                            };
                            let post_participants_result = ml_central_ledger.send(post_participants_request).await?.des().await?;
                            println!("Post participants result:\n{:?}", post_participants_result);

                            let post_initial_position_and_limits_req = PostInitialPositionAndLimits {
                                name: p_args.name.clone(),
                                initial_position_and_limits: InitialPositionAndLimits {
                                    currency: participant_create_args.currency,
                                    limit: Limit {
                                        r#type: LimitType::NetDebitCap,
                                        value: participant_create_args.ndc,
                                    },
                                    initial_position: participant_create_args.position,
                                }
                            };

                            ml_central_ledger.send(post_initial_position_and_limits_req).await?;

                            set_participant_endpoints(
                                &p_args.name,
                                &participant_create_args.url.to_string(),
                                &mut ml_central_ledger,
                            ).await?;
                        },
                    }
                }

                ParticipantSubCommand::Accounts(pa) => {
                    match &pa.subcmd {
                        ParticipantAccountsSubCommand::Fund(part_acc_fund_args) => {
                            let get_accounts = participants::GetDfspAccounts{
                                name: p_args.name.clone(),
                            };
                            let accounts = ml_central_ledger.send(get_accounts).await?.des().await?;
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
                                ParticipantAccountFundSubCommand::In(part_acc_fund_in_args) => {
                                    println!("Not yet implemented")
                                },
                                ParticipantAccountFundSubCommand::Out(part_acc_fund_out_args) => {
                                    println!("Not yet implemented")
                                },
                                ParticipantAccountFundSubCommand::Num(part_acc_fund_num_args) => {
                                    let action = if part_acc_fund_num_args.amount > Amount::ZERO {
                                        participants::ParticipantFundsInOutAction::RecordFundsIn
                                    } else {
                                        participants::ParticipantFundsInOutAction::RecordFundsOutPrepareReserve
                                    };
                                    let funds_request = participants::PostParticipantSettlementFunds {
                                        name: p_args.name,
                                        account_id: account.id,
                                        funds: participants::ParticipantFundsInOut {
                                            transfer_id: fspiox_api::CorrelationId::new(),
                                            action,
                                            amount: fspiox_api::Money {
                                                currency: part_acc_fund_args.currency,
                                                amount: part_acc_fund_num_args.amount.abs()
                                            },
                                            reason: "Voodoo".to_string(),
                                            external_reference: "Voodoo".to_string(),
                                        }
                                    };
                                    ml_central_ledger.send(funds_request).await?;
                                }
                            }
                        }

                        ParticipantAccountsSubCommand::List => {
                            let request = GetDfspAccounts { name: p_args.name };
                            let accounts = ml_central_ledger.send(request).await?.des().await?;
                            // TODO: table
                            for acc in accounts {
                                println!(
                                    "{} {} {} Active: {}",
                                    acc.currency,
                                    acc.ledger_account_type,
                                    acc.value,
                                    (if acc.is_active == 1 { true } else { false }),
                                );
                            }
                        }

                        ParticipantAccountsSubCommand::Enable(acc_enable_args) => {
                            let get_accs_request = GetDfspAccounts { name: p_args.name };
                            let accounts = ml_central_ledger.send(get_accs_request).await?.des().await?;
                            for curr in &acc_enable_args.currency {
                                let currency_acc = accounts.iter().find(|acc|
                                    acc.currency == *curr && acc.ledger_account_type == AnyAccountType::Position
                                );
                                match currency_acc {
                                    Some(acc) => {
                                        let enable_request = PutParticipantAccount {
                                            account_id: acc.id,
                                            name: p_args.name,
                                            set_active: true,
                                        };
                                        ml_central_ledger.send(enable_request).await?;
                                        println!(
                                            "Enabled {} account {} for currency {}",
                                            p_args.name,
                                            acc.id,
                                            curr,
                                        );
                                    }
                                    None => {
                                        println!("Couldn't find account for currency {}", curr);
                                    }
                                }
                            }
                        }

                        ParticipantAccountsSubCommand::Disable(acc_disable_args) => {
                            let get_accs_request = GetDfspAccounts { name: p_args.name };
                            let accounts = ml_central_ledger.send(get_accs_request).await?.des().await?;
                            for curr in &acc_disable_args.currency {
                                let currency_acc = accounts.iter().find(|acc| acc.currency == *curr);
                                match currency_acc {
                                    Some(acc) => {
                                        let enable_request = PutParticipantAccount {
                                            account_id: acc.id,
                                            name: p_args.name,
                                            set_active: false,
                                        };
                                        ml_central_ledger.send(enable_request).await?;
                                        println!(
                                            "Disabled {} account {} for currency {}",
                                            p_args.name,
                                            acc.id,
                                            curr,
                                        );
                                    }
                                    None => {
                                        println!("Couldn't find account for currency {}", curr);
                                    }
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
            use futures::SinkExt;
            use voodoo_doll::Message;

            // TODO: it's really not this module's job to know how to deploy voodoo-doll, where and
            // how to find it once it's deployed, and how to destroy it. That should be delegated
            // to the voodoo-doll module.

            let destroy = voodoo_args.destroy && voodoo_args.subcmd != VoodooSubCommand::Destroy;

            match voodoo_args.subcmd.clone() {
                VoodooSubCommand::Destroy => {
                    voodoo_doll::destroy(Some(pods.clone())).await?;
                }

                VoodooSubCommand::Deploy => {
                    voodoo_doll::create(Some(pods.clone())).await?;
                }

                VoodooSubCommand::Transfer(voodoo_transfer_args) => {
                    let (mut voodoo_write, mut voodoo_read) = voodoo_doll::get_pod_stream(pods.clone()).await?.split();
                    let transfer_id = voodoo_transfer_args.transfer_id.unwrap_or(
                        transfer::TransferId(fspiox_api::CorrelationId::new()));
                    let mut transfers = Vec::new();
                    transfers.push(
                        TransferMessage {
                            msg_sender: voodoo_transfer_args.payer,
                            msg_recipient: voodoo_transfer_args.payee,
                            currency: voodoo_transfer_args.currency,
                            amount: voodoo_transfer_args.amount,
                            transfer_id,
                        }
                    );
                    voodoo_write.send(
                        Message::Text(
                            serde_json::to_string(
                                &ClientMessage::Transfers(
                                    transfers
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
                                    _ => {
                                        // Ignore anything else; we're not interested
                                    }
                                }
                            }
                            _ => {
                                println!("Incoming non-text:");
                                println!("{}", msg);
                            }
                        }
                    }

                    // Cleanup
                    voodoo_write.close().await?;
                }
            }

            if destroy {
                voodoo_doll::destroy(Some(pods)).await?;
            }

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
