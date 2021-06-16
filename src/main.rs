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

use mojaloop_api::fspiox_api::common::{Amount, Currency, FspId};
use std::io::Error;
extern crate clap;
use clap::Clap;

#[derive(Clap)]
#[clap(
    setting = clap::AppSettings::ArgRequiredElseHelp,
    version = clap::crate_version!(),
    name = "Mojaloop CLI"
)]
struct Opts {
    #[clap(short, long)]
    kubernetes_config: Option<String>,
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
    #[clap(subcommand)]
    subcmd: ParticipantSubCommand,
}

#[derive(Clap)]
enum ParticipantSubCommand {
    #[clap(about = "Upsert participant")]
    Upsert(ParticipantUpsert),
}

#[derive(Clap, Debug)]
struct ParticipantUpsert {
    #[clap(index = 1)]
    name: FspId,
    #[clap(short, long)]
    currency: Option<Currency>,
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

#[tokio::main]
async fn main() -> Result<(), Error> {
    let opts: Opts = Opts::parse();

    match opts.subcmd {
        SubCommand::Participant(p) => {
            match p.subcmd {
                ParticipantSubCommand::Upsert(p) => {
                    println!("participant upsert {:?}", p);
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
