# mojaloop-cli
Command-line interface to Mojaloop

`mojaloop-cli` connects directly to your cluster to perform the actions requested of it. It'll use
your current default Kubernetes config- whatever you see when you run `kubectl get pods` is what
`mojaloop-cli` will see and act on. No ingress and no port forwarding required. You can specify the
cluster and namespace:
1. using `-k` or `--kubeconfig` to supply the Kubernetes config file you'd prefer to use
2. running `export KUBECONFIG=/path/to/.kube/config` in your terminal before using this tool
3. using `-n` or `--namespace` to specify the namespace you'd like to target

A simple example creating SEK accounts and a participant in a switch. The output is a little rough
at the time of writing:
```
$ mojaloop-cli hub accounts create all NOK SEK
Created hub reconciliation account: SEK
Created hub settlement account: SEK

$ mojaloop-cli participant testfspsek create SEK 10000 10000
Post participants result:
Participant { name: "testfspsek", id: "http://central-ledger/participants/testfspsek", created: 2021-06-26T16:00:22Z, is_active: 1, accounts: [ParticipantAccount { id: SettlementAccountId(17), ledger_account_type: Position, currency: SEK, is_active: 0 }, ParticipantAccount { id: SettlementAccountId(18), ledger_account_type: Settlement, currency: SEK, is_active: 0 }] }
Post initial position and limits result:
201

$ mojaloop-cli participant testfspsek accounts list
SEK Position 10000
SEK Settlement 0

$ mojaloop-cli participant testfspsek endpoints set all https://testfspsek.io/
Updated FspiopCallbackUrlParticipantBatchPut endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlParticipantBatchPutError endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlParticipantPut endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlParticipantPutError endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlPartiesGet endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlPartiesPut endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlPartiesPutError endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlQuotes endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlTransferError endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlTransferPost endpoint to https://testfspsek.io/. Response 201 Created.
Updated FspiopCallbackUrlTransferPut endpoint to https://testfspsek.io/. Response 201 Created.

$ mojaloop-cli participant testfspsek endpoints list
FspiopCallbackUrlParticipantBatchPut https://testfspsek.io//participants/{{requestId}}
FspiopCallbackUrlParticipantBatchPutError https://testfspsek.io//participants/{{requestId}}/error
FspiopCallbackUrlParticipantPut https://testfspsek.io//participants/{{partyIdType}}/{{partyIdentifier}}
FspiopCallbackUrlParticipantPutError https://testfspsek.io//participants/{{partyIdType}}/{{partyIdentifier}}/error
FspiopCallbackUrlPartiesGet https://testfspsek.io//parties/{{partyIdType}}/{{partyIdentifier}}
FspiopCallbackUrlPartiesPut https://testfspsek.io//parties/{{partyIdType}}/{{partyIdentifier}}
FspiopCallbackUrlPartiesPutError https://testfspsek.io//parties/{{partyIdType}}/{{partyIdentifier}}/error
FspiopCallbackUrlQuotes https://testfspsek.io/
FspiopCallbackUrlTransferError https://testfspsek.io//transfers/{{transferId}}/error
FspiopCallbackUrlTransferPost https://testfspsek.io//transfers
FspiopCallbackUrlTransferPut https://testfspsek.io//transfers/{{transferId}}
```

The current help describes functionality; most, though not all of this exists at present:
```
Mojaloop CLI 0.2.0

USAGE:
    mojaloop-cli [FLAGS] [OPTIONS] <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -j, --json       Produce all output as json
    -V, --version    Prints version information

OPTIONS:
    -k, --kubeconfig <kubeconfig>    Location of the kubeconfig file to use
    -n, --namespace <namespace>      Namespace in which to find the Mojaloop deployment. Defaults to
                                     the default namespace in your kubeconfig, or "default"
    -t, --timeout <timeout>          Per-request timeout. A single command may make multiple
                                     requests [default: 30]

SUBCOMMANDS:
    accounts        Create, read, enable, and disable accounts
    help            Prints this message or the help of the given subcommand(s)
    hub             Hub functions
    participant     Create, read, update, and upsert a single switch participant
    participants    List participants
    quote           Create quotes
    transfer        Execute transfers
```

## Use
Download for your platform from ![releases](https://github.com/partiallyordered/mojaloop-cli/releases).

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
- rename: mojo?
- Allow lower-case currencies? They do "anchor" the commands a little, give them sort of a
    "reference point", in the same way as an upper-case letter does at the beginning of a
    sentence. But they're mildly annoying to type in upper-case. Perhaps it's up to the user to
    "anchor" their commands, or not.
- a "currency" mode, where the user sets an environment variable and no longer needs to supply
    currency arguments. This could be handy, because it's not infrequent to operate a switch in a
    single currency
- simulator creation/configuration?
- ALS configuration?
- reinstate other platforms in CD
- version assertion in GH Actions to prevent releasing a version that doesn't correspond with the
    version in Cargo.toml
- only open port-forward to services actually needed for a given action. I.e. don't open quoting
    service port-forward for central ledger actions.
- use nix for building
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
