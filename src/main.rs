use futures_util::SinkExt as _;
use futures_util::StreamExt as _;
use futures_util::TryStreamExt as _;
use tokio_tungstenite::tungstenite;

#[derive(Debug, serde::Deserialize)]
struct AppsConnectionsOpenResponse {
    ok: bool,
    url: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Value,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    tracing_subscriber::fmt::init();
    let slack_app_token = std::env::var("SLACK_APP_TOKEN")
        .or_else(|_| anyhow::bail!("SLACK_APP_TOKEN is not given"))?;

    let client = reqwest::Client::new();
    let resp: AppsConnectionsOpenResponse = client
        .post("https://slack.com/api/apps.connections.open")
        .bearer_auth(&slack_app_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !resp.ok {
        anyhow::bail!("failed to open connection: {}", resp.rest);
    }
    let url = resp.url.unwrap();
    tracing::info!(%url, "initiated WebSocket mode");
    let (mut ws_stream, resp) =
        tokio_tungstenite::connect_async(format!("{}&debug_reconnects=true", url)).await?;
    tracing::info!(status = %resp.status(), headers = ?resp.headers(), "connected to WebSocket endpoint");

    loop {
        let (mut ws_sink, mut ws_read_stream) = ws_stream.split();

        while let Some(message) = ws_read_stream.try_next().await? {
            match message {
                tungstenite::Message::Ping(payload) => {
                    tracing::info!(?payload, "send a pong in response to ping");
                    ws_sink.send(tungstenite::Message::Pong(payload)).await?;
                }
                tungstenite::Message::Pong(payload) => {
                    tracing::info!(?payload, "received a pong message");
                }
                tungstenite::Message::Text(payload) => {
                    tracing::info!(%payload, "received a text message");
                    if !handle_text(&mut ws_sink, payload).await? {
                        tracing::info!("disconnect is requested");
                        break;
                    }
                }
                tungstenite::Message::Binary(payload) => {
                    tracing::info!(?payload, "received a binary message");
                }
                tungstenite::Message::Close(frame) => {
                    tracing::info!(?frame, "received a close message, closing the stream");
                    break;
                }
            }
        }
        ws_sink.reunite(ws_read_stream)?.close(None).await?;

        tracing::info!("start reconnecting");
        let resp: AppsConnectionsOpenResponse = client
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(&slack_app_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if !resp.ok {
            anyhow::bail!("failed to open connection: {:?}", resp.rest);
        }
        let url = resp.url.unwrap();
        let pair =
            tokio_tungstenite::connect_async(format!("{}&debug_reconnects=true", url)).await?;
        tracing::info!(status = %pair.1.status(), headers = ?pair.1.headers(), "re-connected to WebSocket endpoint");

        ws_stream = pair.0;
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SlackEvent {
    Hello(HelloEvent),
    Disconnect,
    EventsApi(EventsApiEvent),
}

#[derive(Debug, serde::Deserialize)]
struct HelloEvent {
    connection_info: ConnectionInfo,
}

#[derive(Debug, serde::Deserialize)]
struct ConnectionInfo {
    app_id: String,
}

#[derive(Debug, serde::Deserialize)]
struct EventsApiEvent {
    envelope_id: String,
    payload: serde_json::Value,
}

async fn handle_text<S>(ws_sink: &mut S, payload: String) -> anyhow::Result<bool>
where
    S: futures_util::Sink<tungstenite::Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let event: SlackEvent = serde_json::from_str(&payload)?;
    match event {
        SlackEvent::Hello(hello_event) => {
            tracing::info!(app_id = %hello_event.connection_info.app_id);
            Ok(true)
        }
        SlackEvent::EventsApi(events_api_event) => {
            let payload: EventsApiPayload = serde_json::from_value(events_api_event.payload)?;
            let span = tracing::info_span!(
                "EventsApiPayload",
                envelope_id = %events_api_event.envelope_id
            );
            let _guard = span.enter();
            handle_event(payload).await?;

            tracing::info!(envelope_id = %events_api_event.envelope_id, "send an acknowledge");
            ws_sink
                .send(tungstenite::Message::Text(serde_json::to_string(
                    &Acknowledge {
                        envelope_id: events_api_event.envelope_id,
                    },
                )?))
                .await?;
            Ok(true)
        }
        SlackEvent::Disconnect => Ok(false),
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EventsApiPayload {
    EventCallback(EventCallbackPayload),
    #[serde(other)]
    Other,
}

#[derive(Debug, serde::Deserialize)]
struct EventCallbackPayload {
    event: EventCallbackEvent,
    event_id: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EventCallbackEvent {
    Message(MessageEvent),
    #[serde(other)]
    Other,
}

#[derive(Debug)]
enum MessageEvent {
    Plain(PlainMessageEvent),
    Other,
}
impl<'de> serde::Deserialize<'de> for MessageEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct SubtypeTagged {
            subtype: Option<serde_json::Value>,
            #[serde(flatten)]
            value: serde_json::Value,
        }
        let v = SubtypeTagged::deserialize(deserializer)?;
        if v.subtype.is_some() {
            Ok(Self::Other)
        } else {
            Ok(Self::Plain(
                PlainMessageEvent::deserialize(v.value).map_err(serde::de::Error::custom)?,
            ))
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct PlainMessageEvent {
    channel: String,
    thread_ts: Option<String>,
    ts: String,
}

#[derive(Debug, serde::Serialize)]
struct Acknowledge {
    envelope_id: String,
}

#[derive(Debug, serde::Serialize)]
struct ChatGetPermalinkRequest {
    channel: String,
    message_ts: String,
}
#[derive(Debug, serde::Deserialize)]
struct ChatGetPermalinkResponse {
    ok: bool,
    permalink: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
struct ChatPostMessageRequest {
    channel: String,
    text: String,
}
#[derive(Debug, serde::Deserialize)]
struct ChatPostMessageResponse {
    ok: bool,
    ts: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Value,
}

async fn handle_event(payload: EventsApiPayload) -> anyhow::Result<()> {
    let event_callback = match payload {
        EventsApiPayload::EventCallback(ec) => ec,
        EventsApiPayload::Other => {
            tracing::info!("ignore non event_callback type");
            return Ok(());
        }
    };
    let span = tracing::info_span!("MessageEvent", event_id = %event_callback.event_id);
    let _guard = span.enter();

    let message_event = match event_callback.event {
        EventCallbackEvent::Message(me) => me,
        EventCallbackEvent::Other => {
            tracing::info!("ignore non message type");
            return Ok(());
        }
    };
    let event = match message_event {
        MessageEvent::Plain(e) => e,
        MessageEvent::Other => {
            tracing::info!("not a threaded message because subtype is present");
            return Ok(());
        }
    };

    if event.thread_ts.is_none() {
        tracing::info!("not a threaded message because thread_ts is none");
        return Ok(());
    }

    let client = reqwest::Client::new();
    let token = std::env::var("SLACK_OAUTH_TOKEN")?;
    let resp: ChatGetPermalinkResponse = client
        .post("https://slack.com/api/chat.getPermalink")
        .bearer_auth(&token)
        .form(&ChatGetPermalinkRequest {
            channel: event.channel.clone(),
            message_ts: event.ts,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !resp.ok {
        anyhow::bail!("chat.getPermalink failed: {}", resp.rest);
    }
    let permalink = resp.permalink.unwrap();
    tracing::info!(%permalink, "translated to permalink");

    let resp: ChatPostMessageResponse = client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(&token)
        .json(&ChatPostMessageRequest {
            channel: event.channel,
            text: permalink,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !resp.ok {
        anyhow::bail!("chat.postMessage failed: {}", resp.rest);
    }
    let ts = resp.ts.unwrap();
    tracing::info!(%ts, "posted a permalink");
    Ok(())
}