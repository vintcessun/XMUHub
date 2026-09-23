//! Outbound mail (verification codes) through an authenticated SMTP relay —
//! Aliyun DirectMail on 465/implicit TLS, since the host cannot send on port 25.

use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub from: String,
}

impl Mailer {
    pub fn new(cfg: &SmtpConfig) -> anyhow::Result<Mailer> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?
            .port(cfg.port)
            .credentials(Credentials::new(cfg.user.clone(), cfg.password.clone()))
            .timeout(Some(std::time::Duration::from_secs(20)))
            .build();
        let from = format!("XMUHub <{}>", cfg.from).parse()?;
        Ok(Mailer { transport, from })
    }

    pub async fn send_code(&self, to: &str, code: &str, purpose: &str) -> anyhow::Result<()> {
        let (subject, action) = match purpose {
            "reset" => ("XMUHub 找回密码验证码", "重置密码"),
            _ => ("XMUHub 注册验证码", "注册账号"),
        };
        let text = format!(
            "你正在{action}，验证码是：{code}\n\n验证码 10 分钟内有效。如果不是你本人操作，请忽略这封邮件。\n\n—— XMUHub 厦大资料库 https://xmu.vintces.icu"
        );
        let html = format!(
            r#"<div style="font-family:-apple-system,'PingFang SC','Microsoft YaHei',sans-serif;max-width:480px;margin:0 auto;padding:24px;color:#182033">
<div style="font-size:20px;font-weight:700;color:#122e66;margin-bottom:16px">XMUHub · 厦大资料库</div>
<p>你正在{action}，验证码是：</p>
<div style="font-size:32px;font-weight:700;letter-spacing:6px;background:#f0f2f7;border-radius:10px;padding:14px 0;text-align:center;margin:12px 0">{code}</div>
<p style="color:#5f687b;font-size:14px">验证码 10 分钟内有效。如果不是你本人操作，请忽略这封邮件。</p>
<p style="color:#8a93a6;font-size:12px;margin-top:24px">XMUHub 是同学自发维护的非官方资料共享站 · <a href="https://xmu.vintces.icu" style="color:#1d4494">xmu.vintces.icu</a></p>
</div>"#
        );
        let msg = Message::builder()
            .from(self.from.clone())
            .to(to.parse()?)
            .subject(subject)
            .multipart(MultiPart::alternative_plain_html(text, html))?;
        self.transport.send(msg).await?;
        Ok(())
    }
}
