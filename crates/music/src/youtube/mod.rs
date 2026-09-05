mod accounts;
mod auth;
mod client;
mod genres;
mod playback;
mod subscriptions;
mod trim;
mod wire;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use ytmusic::YtMusic;

use crate::youtube::playback::Factory;

use crate::{
    InputSource, MusicProvider, PromptSink, ProviderSession, SignIn, SignInPrompt, UserProfile,
};
pub use client::YouTubeClient;

const GUEST_ID: &str = "youtube-guest";

pub struct YouTubeProvider {
    cookies: PathBuf,
    authuser: PathBuf,
    guest: PathBuf,
    resolved: PathBuf,
    player: PathBuf,
}

impl YouTubeProvider {
    pub fn new() -> Self {
        let cache = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("sonora")
            .join("youtube");
        Self {
            cookies: cache.join("cookies.txt"),
            authuser: cache.join("authuser.txt"),
            guest: cache.join("guest"),
            resolved: cache.join("resolved.json"),
            player: cache.join("player.json"),
        }
    }

    fn cookie_client(&self, cookies: &str, authuser: usize) -> Arc<YtMusic> {
        Arc::new(
            YtMusic::with_cookies(cookies)
                .as_user(authuser)
                .cache_resolutions(self.resolved.clone())
                .cache_player(self.player.clone()),
        )
    }

    fn guest_client(&self) -> Arc<YtMusic> {
        Arc::new(YtMusic::anonymous().cache_player(self.player.clone()))
    }

    fn authenticated_session(&self, api: Arc<YtMusic>, profile: UserProfile) -> ProviderSession {
        let client = YouTubeClient::new(api.clone()).owned_by(profile.display_name.clone());
        ProviderSession {
            profile,
            api: Arc::new(client),
            playback: Arc::new(Factory::new(api)),
            authenticated: true,
            playcounts: false,
        }
    }

    fn guest_session(&self, api: Arc<YtMusic>) -> ProviderSession {
        ProviderSession {
            profile: UserProfile {
                id: GUEST_ID.to_string(),
                display_name: "YouTube Music".to_string(),
            },
            api: Arc::new(YouTubeClient::new(api.clone())),
            playback: Arc::new(Factory::new(api)),
            authenticated: false,
            playcounts: false,
        }
    }

    async fn connect(
        &self,
        cookies: &str,
        prompt: &PromptSink,
        input: &mut InputSource,
    ) -> Result<ProviderSession> {
        let cookies = auth::header(cookies)?;

        let found = accounts::list(&cookies).await;
        let account = match found.len() {
            0 => anyhow::bail!("cookies were not accepted; sign in to the browser first"),
            1 => &found[0],
            _ => pick(&found, prompt, input).await?,
        };

        let profile = wire::profile(account.profile.clone());
        let api = self.cookie_client(&cookies, account.index);
        self.store_cookies(&cookies, account.index)?;
        log::debug!(
            "youtube: cookie sign-in succeeded for authuser {}",
            account.index
        );
        Ok(self.authenticated_session(api, profile))
    }

    fn store_cookies(&self, cookies: &str, authuser: usize) -> Result<()> {
        if let Some(parent) = self.cookies.parent() {
            std::fs::create_dir_all(parent).context("cannot create youtube cache dir")?;
        }
        std::fs::write(&self.cookies, cookies).context("cannot store youtube cookies")?;
        std::fs::write(&self.authuser, authuser.to_string())
            .context("cannot store the youtube account")?;
        let _ = std::fs::remove_file(&self.guest);
        Ok(())
    }

    async fn restore_cookies(&self) -> Option<ProviderSession> {
        let cookies = std::fs::read_to_string(&self.cookies).ok()?;
        let cookies = cookies.trim();
        if cookies.is_empty() {
            return None;
        }
        let authuser = self.stored_authuser();
        let api = self.cookie_client(cookies, authuser);
        match api.profile().await {
            Ok(profile) => {
                log::debug!("youtube: restored the session for authuser {authuser}");
                Some(self.authenticated_session(api, wire::profile(profile)))
            }
            Err(error) => {
                log::warn!("youtube: the cached cookies are no longer usable: {error:#}");
                None
            }
        }
    }

    fn stored_authuser(&self) -> usize {
        std::fs::read_to_string(&self.authuser)
            .ok()
            .and_then(|stored| stored.trim().parse().ok())
            .unwrap_or(0)
    }

    fn store_guest(&self) {
        if let Some(parent) = self.guest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.guest, b"");
    }
}

async fn pick<'a>(
    found: &'a [accounts::Account],
    prompt: &PromptSink,
    input: &mut InputSource,
) -> Result<&'a accounts::Account> {
    prompt(SignInPrompt::Accounts(
        found.iter().map(accounts::Account::choice).collect(),
    ));
    let picked = input.recv().await.context("sign-in was cancelled")?;
    found
        .iter()
        .find(|account| account.index.to_string() == picked.trim())
        .context("that account is no longer signed in")
}

impl Default for YouTubeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MusicProvider for YouTubeProvider {
    fn name(&self) -> &'static str {
        "YouTube Music"
    }

    fn slug(&self) -> &'static str {
        "youtube"
    }

    fn sign_in_options(&self) -> Vec<SignIn> {
        vec![SignIn::Anonymous, SignIn::Secret]
    }

    fn stored(&self) -> bool {
        self.cookies.exists() || self.guest.exists()
    }

    async fn restore(&self) -> Result<Option<ProviderSession>> {
        if let Some(session) = self.restore_cookies().await {
            return Ok(Some(session));
        }
        if self.guest.exists() {
            log::debug!("youtube: restoring guest session");
            return Ok(Some(self.guest_session(self.guest_client())));
        }
        Ok(None)
    }

    async fn sign_in(
        &self,
        method: SignIn,
        prompt: crate::PromptSink,
        mut input: InputSource,
    ) -> Result<ProviderSession> {
        match method {
            SignIn::Anonymous | SignIn::Default => {
                self.store_guest();
                Ok(self.guest_session(self.guest_client()))
            }
            SignIn::Secret => {
                prompt(SignInPrompt::Secret);
                let cookies = input.recv().await.context("sign-in was cancelled")?;
                self.connect(&cookies, &prompt, &mut input).await
            }
            SignIn::Path(_) => Err(anyhow::anyhow!(
                "youtube does not sign in with a folder path"
            )),
        }
    }

    fn sign_out(&self) {
        for path in [&self.cookies, &self.authuser, &self.guest] {
            if let Err(error) = std::fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("youtube: cannot remove credential cache: {error}");
            }
        }
    }
}
