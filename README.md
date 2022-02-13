# slack-thread-expander
Expand threaded messages without "Also sent to the channel"

![](https://gyazo.wanko.cc/45deb54c1d3a5b2201336889a0ae8ad8.png)

## Usage
### Setup Slack App
Create a Slack App for slack-thread-expander.
You can use [app_manifest.yml](./app_manifest.yml) to create and configure the Slack App. See https://api.slack.com/reference/manifests for details.

### Setup environment variables for slack-thread-expander
- `SLACK_APP_TOKEN`
  - App-Level token https://api.slack.com/authentication/token-types#app
- `SLACK_OAUTH_TOKEN`
  - Bot user OAuth token https://api.slack.com/authentication/token-types#bot

### Run slack-thread-expander
```
% cargo build --release
% SLACK_APP_TOKEN=... SLACK_OAUTH_TOKEN=... target/release/slack-thread-expander
```

### Use slack-thread-expander
Invite the Bot user to the target channel. All threaded messages in channels which the bot user joins are expanded.
