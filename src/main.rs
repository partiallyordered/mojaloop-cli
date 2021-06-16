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
                        .long("currency")
                        .short('c')
                        .takes_value(true)
                )
                .arg(
                    Arg::new("ndc")
                        .about("The participant account NDC")
                        .long("ndc")
                        .short('n')
                        .takes_value(true)
                        .requires("currency")
                )
                .arg(
                    Arg::new("position")
                        .about("The participant account position")
                        .long("position")
                        .short('p')
                        .takes_value(true)
                        .requires("currency")
                )
            )
        )
        .get_matches();

    match args.subcommand() {
        Some(("participant", participant_args)) => {
            match participant_args.subcommand() {
                Some(("upsert", upsert_args)) => {
                    println!("upsert args {:?}", upsert_args);
                },
                _ => println!("participant args {:?}", participant_args)
            }
        },
        _ => ()
    }

    Ok(())
}
