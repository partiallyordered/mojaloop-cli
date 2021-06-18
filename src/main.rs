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
    fspiox_api::common::{Amount, Currency, FspId, ErrorResponse},
};
use std::convert::TryInto;
extern crate clap;
use clap::Clap;

use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::apps::v1::Deployment;
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
enum MojaloopCliError {
    #[error("Couldn't find central ledger admin pod")]
    CentralLedgerAdminPodNotFound,
    #[error("Unexpected central ledger pod implementation")]
    UnexpectedCentralLedgerPodImplementation,
    #[error("Central ledger service container not found in pod")]
    CentralLedgerServiceContainerNotFound,
    #[error("Ports not present on central ledger service container")]
    CentralLedgerServicePortNotFound,
    #[error("Couldn't retrieve pod list from cluster")]
    ClusterConnectionError,
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

async fn get_central_ledger_port_forward(client: Client) ->
    anyhow::Result<(impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin)>
{
    // Find a single pod with the following label and container name. Port-forward the port with
    // the following port name.
    let label = "app.kubernetes.io/name=centralledger-service";
    let container_name = "centralledger-service";
    let port_name = "http-api";

    // TODO: this namespace should come from the program opts
    let pods: Api<Pod> = Api::namespaced(client, "default");
    let lp = ListParams::default().labels(&label);
    let central_ledger_pod = pods
        .list(&lp).await.map_err(|_| MojaloopCliError::ClusterConnectionError)? // TODO: test connection error (or whatever might occur here- read the source)
        .items.get(0).ok_or(MojaloopCliError::CentralLedgerAdminPodNotFound)?.clone();
    let central_ledger_pod_name = central_ledger_pod.metadata.name.clone().unwrap();
    let central_ledger_port = central_ledger_pod
        .spec.ok_or(MojaloopCliError::UnexpectedCentralLedgerPodImplementation)?
        .containers.iter().find(|c| c.name == container_name).ok_or(MojaloopCliError::CentralLedgerServiceContainerNotFound)?
        .ports.as_ref().ok_or(MojaloopCliError::CentralLedgerServicePortNotFound)?
        .iter().find(|p| p.name.as_ref().map_or(false, |name| name == port_name)).ok_or(MojaloopCliError::CentralLedgerServicePortNotFound)?.clone();
    println!("Central ledger pod name: {}", central_ledger_pod_name);
    println!("Central ledger port: {:?}", central_ledger_port);

    let mut pf = pods.portforward(
        &central_ledger_pod_name,
        &[central_ledger_port.container_port.try_into().unwrap()]
    ).await?;
    let mut ports = pf.ports().unwrap();
    let port = ports[0].stream().unwrap();
    Ok(port)
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

    let client = Client::try_default().await?;
    let central_ledger_conn = get_central_ledger_port_forward(client).await?;

    let (mut request_sender, connection) = Builder::new()
        .handshake(central_ledger_conn)
        .await?;

    // spawn a task to poll the connection and drive the HTTP state
    tokio::spawn(async move {
        if let Err(e) = connection.await {
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
                                        create_hub_account(&mut request_sender, *currency, HubAccountType::HubReconciliation).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::Sett(hub_accs_create_sett_args) => {
                                    for currency in &hub_accs_create_sett_args.currencies {
                                        create_hub_account(&mut request_sender, *currency, HubAccountType::HubMultilateralSettlement).await?;
                                    }
                                }
                                HubAccountsCreateSubCommand::All(hub_accs_create_all_args) => {
                                    for currency in &hub_accs_create_all_args.currencies {
                                        create_hub_account(&mut request_sender, *currency, HubAccountType::HubReconciliation).await?;
                                        create_hub_account(&mut request_sender, *currency, HubAccountType::HubMultilateralSettlement).await?;
                                    }
                                }
                            }
                        }
                        HubAccountsSubCommand::List => {
                            // TODO: might need to take hub name as a parameter, in order to
                            // support newer and older hub names of "hub" and "Hub"? Or just don't
                            // support old hub name? Or just try both?
                            let request = to_hyper_request(GetDfspAccounts { name: "Hub".to_string() })?;
                            let (_, accounts) = send_hyper_request::<DfspAccounts>(&mut request_sender, request).await?;
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
                    let (_, participants) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participants>(&mut request_sender, request).await?;

                    // TODO: additional CLI parameters to get more information about participants

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
                    let (_, existing_participants) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participants>(&mut request_sender, request).await?;

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
                            let (_, post_participants_result) = send_hyper_request::<mojaloop_api::central_ledger::participants::Participant>(&mut request_sender, post_participants_request).await?;
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
                            let (result, _) = send_hyper_request_no_response_body(&mut request_sender, post_initial_position_and_limits_req).await?;
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
                            let (_, accounts) = send_hyper_request::<DfspAccounts>(&mut request_sender, request).await?;
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
