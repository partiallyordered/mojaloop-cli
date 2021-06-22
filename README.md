# mojaloop-cli
Command-line interface to Mojaloop

## Build
```sh
cargo b
```
### With Nix
```sh
nix-shell --run 'cargo b'
```

## Release
```sh
git tag vX.Y.Z # replace X, Y, Z with something appropriate, preferably the version in Cargo
git push --tags
```

### TODO
- puppet should be able to "hijack" fsps temporarily by
  1. getting their endpoints and storing them locally
  2. modifying their endpoints so that it becomes the endpoint for those fsps
  3. running a sequence of transfers that it manages
  4. restoring their endpoints to the endpoints stored in (1)
  This means it'll be able to control transfers on a switch that has simulators. This "hijack"
  functionality should perhaps be behind a flag "--hijack" or something, so the user sort of knows.
- a "initial-setup" subcommand or similar, intended to preconfigure the switch with
  - hub accounts in a certain currency or currencies (probably just one?)
  - a number of participants with
    - endpoints (probably from a template)
    - accounts in the "switch currency/ies"
  Make this command idempotent, or at least able to ignore certain errors, so it's possible to run
  it for multiple currencies, or to "add this functionality/these-dfsps to this switch"
- better logging, a verbose mode of some sort. Probably use slog to enable logs to be printed as
    json in json mode, and text in text mode.
- put git revision (and possibly link to repo at that revision) in -v flag
- check out https://docs.rs/clap/3.0.0-beta.2/clap/enum.AppSettings.html
- try `mojaloop-cli somethingthatdoesnotexist`- is the output sensible? useful?
    - what is this? "If you tried to supply `what` as a PATTERN use `-- what`"
- docs
  - is it possible to supply multiple subcommands? e.g.
    - `mojaloop-cli participants create testfsp1 accounts create testfsp1 EUR fundsin testfsp1 EUR 1000`
- shell autocomplete (and usage doc)
- make a `puppeteer` subcommand. Create the entire subcommand as a CLI client in the
    `mojaloop-ws-adapter` repo. Import it here as the `puppeteer` subcommand. This way the
    puppeteer primitives and CLI can be maintained in that repo, and this can be a slightly more
    general tool that doesn't require a puppeteer instance in the cluster, except when the
    puppeteer subcommand is used.
- raw JSON output with -j
- `mojaloop-cli participant somenonexistentparticipant create INR` can fail as follows:
    ```
    Error: Mojaloop API error: {"errorInformation":{"errorCode":"3003","errorDescription":"Add Party information error - Hub reconciliation account for the specified currency does not exist"}}
    ```
    when the Hub doesn't have an account in the correct currency. We should hint at the user as to
    how to resolve this. And possibly provide the option of creating a hun reconciliation account
    for said currency. Why ML doesn't just automatically have an account in each currency I do not
    know. Could be worth raising an issue.
- build for various platforms in CI, publish binaries to GH releases. See if it's possible to have
    the released binary have the execute bit already set. Also, provide instructions for the
    easiest possible way of running it.
- build and publish a docker image containing only the cli so people can use it from docker run.
    Publish to GHCR and dockerhub.
- asciinema demo
- take a role in actual deployment? I.e. assist users to get a DO/minikube cluster with ML
    deployed?
- `mojaloop-cli hub accounts create sett EUR GBP ZZZ`
    produces:
    ```
    error: Invalid value for '<currencies>...': Matching variant not found

    For more information try --help
    ```
    because the `ZZZ` currency is missing. This is a moderately opaque error message.
- can Clap use types to provide help information? Can we, for instance, provide some impls on
    foreign types to provide default help information? For example on Currency or Amount types?
