use std::time::Duration;

use anyhow::ensure;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{config::MailConfig, repositories::user_repository::UserRepository};

use super::worker::JobError;

pub const KIND: &str = "send_welcome_email";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WelcomePayload {
    pub user_id: Uuid,
}

pub(crate) struct WelcomeEmailHandler {
    delivery: Delivery,
}

enum Delivery {
    Log,
    Smtp {
        sender: Mailbox,
        client: Box<AsyncSmtpTransport<Tokio1Executor>>,
    },
}

impl WelcomeEmailHandler {
    pub(crate) fn new(config: &MailConfig) -> anyhow::Result<Self> {
        let delivery = match config.transport.as_str() {
            "log" => Delivery::Log,
            "smtp" => {
                ensure!(
                    !config.smtp_host.trim().is_empty() && config.smtp_port > 0,
                    "SMTP host and port are required"
                );
                ensure!(
                    config.smtp_username.is_empty() == config.smtp_password.is_empty(),
                    "SMTP username and password must both be configured or both be empty"
                );
                let sender = config
                    .from
                    .parse::<Mailbox>()
                    .map_err(|_| anyhow::anyhow!("invalid sender mailbox configuration"))?;
                // Port 465 uses implicit TLS. Other configured ports require a
                // successful STARTTLS upgrade before any credentials or mail.
                let builder = if config.smtp_port == 465 {
                    AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)
                } else {
                    AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
                }
                .map_err(|_| anyhow::anyhow!("could not configure SMTP TLS transport"))?;
                let mut builder = builder
                    .port(config.smtp_port)
                    .timeout(Some(Duration::from_secs(20)));
                if !config.smtp_username.is_empty() {
                    builder = builder.credentials(Credentials::new(
                        config.smtp_username.clone(),
                        config.smtp_password.clone(),
                    ));
                }
                Delivery::Smtp {
                    sender,
                    client: Box::new(builder.build()),
                }
            }
            _ => anyhow::bail!("mail transport must be log or smtp"),
        };
        Ok(Self { delivery })
    }

    pub(crate) async fn handle(
        &self,
        repository: &dyn UserRepository,
        job_id: Uuid,
        payload: &Value,
    ) -> Result<(), JobError> {
        let payload: WelcomePayload =
            serde_json::from_value(payload.clone()).map_err(|_| JobError::InvalidPayload)?;
        let Some(user) = repository
            .find_by_id(payload.user_id)
            .await
            .map_err(|_| JobError::UserLookup)?
        else {
            // A deleted account should not be contacted or retried forever.
            tracing::info!(%job_id, user_id = %payload.user_id, "welcome job skipped for deleted user");
            return Ok(());
        };
        match &self.delivery {
            Delivery::Log => {
                tracing::info!(%job_id, user_id = %user.id, transport = "log", "welcome email simulated; log transport does not deliver email");
            }
            Delivery::Smtp { sender, client } => {
                let recipient: Mailbox =
                    user.email.parse().map_err(|_| JobError::InvalidRecipient)?;
                let message = Message::builder()
                    .from(sender.clone())
                    .to(recipient)
                    .subject("Welcome")
                    // A stable Message-ID may help downstream deduplication, but
                    // SMTP still provides no exactly-once delivery guarantee.
                    .message_id(Some(format!(
                        "<welcome-{}@jobs.rust-api-skeleton.invalid>",
                        user.id
                    )))
                    .header(ContentType::TEXT_PLAIN)
                    .body(format!(
                        "Welcome, {}!\n\nYour account is ready.\n",
                        user.name
                    ))
                    .map_err(|_| JobError::MessageBuild)?;
                // Never propagate the SMTP error: servers may echo credentials,
                // recipients, or message content in their diagnostic text.
                client
                    .send(message)
                    .await
                    .map_err(|_| JobError::SmtpDelivery)?;
                tracing::info!(%job_id, user_id = %user.id, transport = "smtp", "welcome email accepted by SMTP server");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_mail_config_does_not_echo_sensitive_values() {
        let config = MailConfig {
            transport: "smtp".into(),
            from: "recipient-secret-invalid-mailbox".into(),
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            smtp_username: "private-username".into(),
            smtp_password: "private-password".into(),
        };
        let error = WelcomeEmailHandler::new(&config)
            .err()
            .expect("invalid mailbox must fail");
        let diagnostic = format!("{error:?}");
        for secret in ["recipient-secret", "private-username", "private-password"] {
            assert!(!diagnostic.contains(secret));
        }
    }
}
