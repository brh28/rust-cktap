/// CLI for rust-cktap
use clap::{Parser, Subcommand};
use rpassword::read_password;
use rust_cktap::commands::{CkTransport, Read};
use rust_cktap::{apdu::Error, commands::Certificate, pcsc, CkTapCard};
use std::io;
use std::io::Write;

/// SatsCard CLI
#[derive(Parser)]
#[command(author, version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"), about,
long_about = None, propagate_version = true)]
struct SatsCardCli {
    #[command(subcommand)]
    command: SatsCardCommand,
}

/// Commands supported by SatsCard cards
#[derive(Subcommand)]
enum SatsCardCommand {
    /// Show the card status
    Debug,
    /// Show current deposit address
    Address,
    /// Check this card was made by Coinkite: Verifies a certificate chain up to root factory key.
    Certs,
    /// Read the pubkey
    Read,
    /// Derive a public key at the m/0 path, the first non-hardened derived key
    Derive,
}

/// TapSigner CLI
#[derive(Parser)]
#[command(author, version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"), about,
long_about = None, propagate_version = true)]
struct TapSignerCli {
    #[command(subcommand)]
    command: TapSignerCommand,
}

/// Commands supported by TapSigner cards
#[derive(Subcommand)]
enum TapSignerCommand {
    /// Show the card status
    Debug,
    /// Check this card was made by Coinkite: Verifies a certificate chain up to root factory key.
    Certs,
    /// Read the pubkey (requires CVC)
    Read,
    /// Derive a public key at the given hardened path
    Derive {
        /// path, eg. for 84'/0'/0'/* use 84,0,0
        #[clap(short, long, value_delimiter = ',', num_args = 1..)]
        path: Vec<u32>,
    },
}

fn main() -> Result<(), Error> {
    // figure out what type of card we have before parsing cli args
    let mut card = pcsc::find_first()?;

    match &mut card {
        CkTapCard::SatsCard(sc) => {
            let cli = SatsCardCli::parse();
            match cli.command {
                SatsCardCommand::Debug => {
                    dbg!(&sc);
                }
                SatsCardCommand::Address => println!("Address: {}", sc.address().unwrap()),
                SatsCardCommand::Certs => check_cert(sc),
                SatsCardCommand::Read => read(sc, None),
                SatsCardCommand::Derive => {
                    let response = &sc.derive();
                    dbg!(response);
                }
            }
        }
        CkTapCard::TapSigner(ts) | CkTapCard::SatsChip(ts) => {
            let cli = TapSignerCli::parse();
            match cli.command {
                TapSignerCommand::Debug => {
                    dbg!(&ts);
                }
                TapSignerCommand::Certs => check_cert(ts),
                TapSignerCommand::Read => read(ts, Some(cvc())),
                TapSignerCommand::Derive { path } => {
                    let response = &ts.derive(path, cvc());
                    dbg!(response);
                }
            }
        }
    }

    Ok(())
}

// handler functions for each command

fn check_cert<T: CkTransport>(card: &mut dyn Certificate<T>) {
    if let Ok(k) = card.check_certificate() {
        println!(
            "Genuine card from Coinkite.\nHas cert signed by: {}",
            k.name()
        )
    } else {
        println!("Card failed to verify. Not a genuine card")
    }
}

fn read<T: CkTransport>(card: &mut dyn Read<T>, cvc: Option<String>) {
    match card.read(cvc) {
        Ok(resp) => println!("{}", resp),
        Err(e) => {
            dbg!(&e);
            println!("Failed to read with error: ")
        }
    }
}

fn cvc() -> String {
    print!("Enter cvc: ");
    io::stdout().flush().unwrap();
    let cvc = read_password().unwrap();
    cvc.trim().to_string()
}
