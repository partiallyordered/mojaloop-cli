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
    common::{to_hyper_request, resp_from_raw_http, resp_headers_from_raw_http, req_to_raw_http, Response, MlApiErr},
    central_ledger::participants::{GetParticipants, Limit, LimitType, PostParticipant, InitialPositionAndLimits, GetDfspAccounts, DfspAccounts, PostInitialPositionAndLimits, NewParticipant},
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

#[derive(Clap)]
#[clap(
    setting = clap::AppSettings::ArgRequiredElseHelp,
    version = clap::crate_version!(),
    name = "Mojaloop CLI"
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
    /// Namespace in which to find the Mojaloop deployment
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
    #[clap(about = "Create, read, update, and upsert a single switch participant")]
    Participant(Participant),
    #[clap(about = "Create, read, enable, and disable accounts")]
    Accounts(Accounts),
    #[clap(about = "List participants (for now)")]
    Participants(Participants),
}

#[derive(Clap)]
struct Participants {
    #[clap(subcommand)]
    subcmd: ParticipantsSubCommand,
}

#[derive(Clap)]
enum ParticipantsSubCommand {
    #[clap(about = "List participants")]
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
    #[clap(about = "Modify participant account")]
    Accounts(ParticipantAccount),
    #[clap(about = "Create a participant")]
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
    #[clap(about = "Upsert participant account")]
    Upsert(ParticipantAccountUpsert),
    #[clap(about = "List participant accounts")]
    List,
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
    #[clap(about = "Create accounts")]
    Create(AccountsCreate),
}

#[derive(Clap, Debug)]
struct AccountsCreate {
    #[clap(index = 1)]
    participant_name: FspId,
    #[clap(index = 2)]
    currency: Currency,
}

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
    // Implemented as tokio::io::DuplexStream
    let port = ports[0].stream().unwrap();
    Ok(port)
}

// TODO: is it nicer to just call a different method if we don't want to parse the body? Instead of
// returning a Result<Option<Resp>> we could return a Result<Resp>. The easy way to do this is to
// split the function in half. We should also return the response _and_ the body, so the user can
// use the response code and headers if they wish.
async fn send_hyper_request<Resp>(
    request_sender: &mut hyper::client::conn::SendRequest<hyper::body::Body>,
    req: hyper::Request<hyper::body::Body>
) -> Result<Option<Resp>, serde_json::Error>
where
    Resp: serde::de::DeserializeOwned,
{
    // TODO: handle unwraps properly
    let response = request_sender.send_request(req).await.unwrap();
    assert!(response.status().is_success(), "{:?}", response);

    // TODO:
    // 1. this code can be better using option/result methods.
    // 2. we can't really just not parse the body if the content length is zero, because content
    //    length header is optional.
    if let Some(length) = response.headers().get(hyper::header::CONTENT_LENGTH) {
        if length != "0" {
            let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
            serde_json::from_slice::<Resp>(&body).map(|res| Some(res))
        } else {
            Ok(None)
        }
    }
    else {
        Ok(None)
    }
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
    // (i.e. "I want this config") easier.
    // let operations = match opts.subcmd {
    match opts.subcmd {
        SubCommand::Participants(ps_args) => {
            match ps_args.subcmd {
                ParticipantsSubCommand::List => {
                    use cli_table::{print_stdout, Cell, Table};

                    let request = to_hyper_request(GetParticipants {})?;
                    let participants = send_hyper_request::<mojaloop_api::central_ledger::participants::Participants>(&mut request_sender, request).await?.unwrap();

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
                    let existing_participants = send_hyper_request::<mojaloop_api::central_ledger::participants::Participants>(&mut request_sender, request).await?.unwrap();

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
                            let post_participants_result = send_hyper_request::<mojaloop_api::central_ledger::participants::Participant>(&mut request_sender, post_participants_request).await?.unwrap();
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
                            let result = send_hyper_request::<()>(&mut request_sender, post_initial_position_and_limits_req).await?;
                            println!("Post initial position and limits result:\n{:?}", result);

                        },
                    }
                }
                ParticipantSubCommand::Accounts(pa) => {
                    match &pa.subcmd {
                        ParticipantAccountsSubCommand::List => {
                            let request = to_hyper_request(GetDfspAccounts { name: p_args.name })?;
                            let accounts = send_hyper_request::<DfspAccounts>(&mut request_sender, request).await?.unwrap();
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
