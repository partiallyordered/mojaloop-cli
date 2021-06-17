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
    common::{resp_from_raw_http, req_to_raw_http, Response, MlApiErr},
    central_ledger::participants::{GetParticipants, Participants},
    fspiox_api::common::{Amount, Currency, FspId, ErrorResponse},
};
use std::convert::TryInto;
extern crate clap;
use clap::Clap;

use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::apps::v1::Deployment;
use thiserror::Error;

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
    #[clap(short, long)]
    kubeconfig: Option<String>,
    // TODO: all namespace option? Don't have a reserved "all" argument i.e. --namespace=all,
    // because someone could call their real namespace "all". Probably try to go with common k8s
    // flags for this, perhaps -A and --all-namespaces (check those are correct).
    #[clap(short, long)]
    namespace: Option<String>,
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Clap)]
enum SubCommand {
    #[clap(about = "Create, read, update, and upsert a single switch participant")]
    Participant(Participant),
    #[clap(about = "Create, read, enable, and disable accounts")]
    Accounts(Accounts),
}

#[derive(Clap)]
struct Participant {
    #[clap(index = 1)]
    name: FspId,
    #[clap(subcommand)]
    subcmd: ParticipantSubCommand,
}

#[derive(Clap)]
enum ParticipantSubCommand {
    #[clap(about = "Modify participant account")]
    Account(ParticipantAccount),
    // #[clap(about = "Upsert participant")]
    // Upsert(ParticipantAccountUpsert),
    // Create(ParticipantCreate),
}

#[derive(Clap)]
struct ParticipantAccount {
    #[clap(subcommand)]
    subcmd: ParticipantAccountSubCommand,
}

#[derive(Clap)]
enum ParticipantAccountSubCommand {
    #[clap(about = "Upsert participant account")]
    Upsert(ParticipantAccountUpsert),
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

async fn get_central_ledger_port_forward(client: Client) -> anyhow::Result<(impl tokio::io::AsyncRead+tokio::io::AsyncWrite+Unpin)> {
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // TODO: collect a list of actions to take, then pass them to a function that takes those
    // actions. This will make a --dry-run option easier. It will also make a declarative format
    // (i.e. "I want this config") easier.
    // TODO: don't connect to k8s until after the opts have been parsed
    let client = Client::try_default().await?;

    let mut port = get_central_ledger_port_forward(client).await?;
    let get_participants = GetParticipants {};
    // let data = b"GET /participants HTTP/1.1\r\nConnection: close\r\nAccept: application/json\r\n\r\n";
    let data = req_to_raw_http(get_participants);
    println!("{}", String::from_utf8(data.clone()).unwrap());
    port.write_all(&data).await?;
    let mut rstream = tokio_util::io::ReaderStream::new(port);
    if let Some(res) = rstream.next().await {
        match res {
            Ok(bytes) => {
                // println!("{:?}", resp_from_raw_http::<Participants>(&bytes[..]));
                let resp = resp_from_raw_http::<Participants>(&bytes[..])?;
                println!("{:?}", resp);
            }
            Err(err) => eprintln!("{:?}", err),
        }
    }

    let opts: Opts = Opts::parse();

    match opts.subcmd {
        SubCommand::Participant(p) => {
            match p.subcmd {
                ParticipantSubCommand::Account(pa) => {
                    match pa.subcmd {
                        ParticipantAccountSubCommand::Upsert(acc) => {
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
