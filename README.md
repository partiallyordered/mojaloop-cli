# mojaloop-cli
Command-line interface to Mojaloop

### TODO
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
