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

use std::io::Error;
extern crate clap;
use clap::{Arg, App, crate_version, AppSettings};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = App::new("Mojaloop CLI")
        .version(crate_version!())
        .about("A command-line interface to the Mojaloop API")
        .setting(AppSettings::ArgRequiredElseHelp)
        .subcommand(App::new("accounts")
            .about("Create, read, update, and fund switch accounts")
            .setting(AppSettings::ArgRequiredElseHelp)
            .subcommand(App::new("create")
                .about("Create a switch account")
                .arg(
                    Arg::new("participant name")
                        .about("The participant name")
                        .index(1)
                        .requires("currency")
                        .required(true)
                    )
                .arg(
                    Arg::new("currency")
                        .about("The account currency")
                        .index(2)
                        .multiple(true)
                )
            )
        )
        .subcommand(App::new("participants")
            .about("Create, read, and update switch participants")
            .setting(AppSettings::ArgRequiredElseHelp)
            .subcommand(App::new("create")
                .about("Create participants")
                .arg(
                    Arg::new("name")
                        .about("The participant name")
                        .index(1)
                        .multiple(true)
                        .required(true)
                )
            )
        )
        .subcommand(App::new("participant")
            .about("Create, read, update, and upsert a single switch participant. More granular than the `participants` subcommand.")
            .setting(AppSettings::ArgRequiredElseHelp)
            .subcommand(App::new("upsert")
                .about("Upsert participant")
                .arg(
                    Arg::new("name")
                        .about("The participant name")
                        .index(1)
                        .required(true)
                )
                .arg(
                    Arg::new("currency")
                        .about("The participant account currency")
                        .index(2)
                        .required(true)
                )
                .arg(
                    Arg::new("ndc")
                        .about("The participant account NDC")
                        .index(3)
                        .required(true)
                )
                .arg(
                    Arg::new("position")
                        .about("The participant account position")
                        .index(4)
                        .required(true)
                )
            )
        )
        .get_matches();

    if let Some(ref participants_args) = args.subcommand_matches("participants") {
        println!("participants {:?}", participants_args);
    }

    println!("{:?}", args);
    Ok(())
}
