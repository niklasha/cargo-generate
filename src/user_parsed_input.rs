//! Input from user but after prase

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use console::style;
use regex::Regex;

use crate::{app_config::AppConfig, warn, GenerateArgs};

// Contains parsed information from user.
pub struct UserParsedInput {
    // from where clone or copy template?
    template_location: TemplateLocation,
    // if template_location contains many templates user already specified one
    subfolder: Option<String>,
    // all values that user defined through:
    // 1. envirnoment variables
    // 2. configuration file
    // 3. cli arguments --define
    template_values: HashMap<String, toml::Value>,
    //TODO:
    // 1. This structure should be used instead of args
    // 2. This struct can contains internaly args and app_config to not confuse
    //    other developer with parsing configuration and args by themself
}

impl UserParsedInput {
    fn new<T: AsRef<str>>(
        template_location: impl Into<TemplateLocation>,
        subfolder: Option<T>,
        default_values: HashMap<String, toml::Value>,
    ) -> Self {
        Self {
            template_location: template_location.into(),
            subfolder: subfolder.map(|s| s.as_ref().to_owned()),
            template_values: default_values,
        }
    }

    /// Try create `UserParsedInput` reading in order [`AppConfig`] and [`Args`]
    ///
    /// # Panics
    /// This function assume that Args and AppConfig are verfied eariler and are logicly correct
    /// For example if both `--git` and `--path` are set this function will panic
    pub fn try_from_args_and_config(app_config: &AppConfig, args: &GenerateArgs) -> Self {
        let mut default_values = app_config.values.clone().unwrap_or_default();
        let ssh_identity = app_config
            .defaults
            .as_ref()
            .and_then(|dcfg| dcfg.ssh_identity.clone())
            .or_else(|| args.ssh_identity.clone());

        // --git
        if let Some(git_url) = args.template_path.git() {
            let git_user_in = GitUserInput::new(
                git_url,
                args.template_path.branch(),
                ssh_identity,
                args.force_git_init,
            );
            return Self::new(git_user_in, args.template_path.subfolder(), default_values);
        }

        // --path
        if let Some(path) = args.template_path.path() {
            return Self::new(
                path.as_ref(),
                args.template_path.subfolder(),
                default_values,
            );
        }

        // check if favorite is favorite configuration
        let fav_name = args.template_path.any_path();

        if let Some(fav_cfg) = app_config.get_favorite_cfg(fav_name) {
            assert!(fav_cfg.git.is_none() || fav_cfg.path.is_none());

            let temp_location = fav_cfg.git.as_ref().map_or_else(
                || fav_cfg.path.as_ref().map(TemplateLocation::from).unwrap(),
                |git_url| {
                    let branch = args
                        .template_path
                        .branch()
                        .map(|s| s.as_ref().to_owned())
                        .or_else(|| fav_cfg.branch.clone());
                    let git_user_input = GitUserInput::new(
                        git_url,
                        branch.as_ref(),
                        ssh_identity,
                        args.force_git_init,
                    );

                    TemplateLocation::from(git_user_input)
                },
            );

            if let Some(fav_default_values) = &fav_cfg.values {
                default_values.extend(fav_default_values.clone());
            }

            return Self::new(
                temp_location,
                args.template_path
                    .subfolder()
                    .map(|s| s.as_ref().to_owned())
                    .or_else(|| fav_cfg.subfolder.clone()),
                default_values,
            );
        }

        // there is no specified favorite in configuration
        // this part try to guess what user wanted in order:

        // 1. look for abbrevations like gh:, gl: etc.
        let temp_location = abbreviated_git_url_to_full_remote(&fav_name).map(|git_url| {
            let git_user_in = GitUserInput::with_git_url_and_args(&git_url, args);
            TemplateLocation::from(git_user_in)
        });

        // 2. check if template directory exist
        let temp_location =
            temp_location.or_else(|| local_path(fav_name).map(TemplateLocation::from));

        // 3. check if the input is in form org/repo<> (map to github)
        let temp_location = temp_location.or_else(|| {
            abbreviated_github(fav_name).map(|git_url| {
                let git_user_in = GitUserInput::with_git_url_and_args(&git_url, args);
                TemplateLocation::from(git_user_in)
            })
        });

        // 4. assume user wanted use --git
        let temp_location = temp_location.unwrap_or_else(|| {
            let git_user_in = GitUserInput::new(
                &fav_name,
                args.template_path.branch(),
                ssh_identity,
                args.force_git_init,
            );
            TemplateLocation::from(git_user_in)
        });

        // Print information what happend to user
        let location_msg = match &temp_location {
            TemplateLocation::Git(git_user_input) => {
                format!("git repository: {}", style(git_user_input.url()).bold())
            }
            TemplateLocation::Path(path) => {
                format!("local path: {}", style(path.display()).bold())
            }
        };
        warn!(
            "Favorite `{}` not found in config, using it as a {}",
            style(&fav_name).bold(),
            location_msg
        );

        Self::new(
            temp_location,
            args.template_path
                .subfolder()
                .map(|s| s.as_ref().to_owned()),
            default_values,
        )
    }

    pub const fn location(&self) -> &TemplateLocation {
        &self.template_location
    }

    pub fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }

    pub const fn template_values(&self) -> &HashMap<String, toml::Value> {
        &self.template_values
    }

    pub fn template_values_mut(&mut self) -> &mut HashMap<String, toml::Value> {
        &mut self.template_values
    }
}

// favorite can be in form with abbrevation what means that input is git repositoru
pub fn abbreviated_git_url_to_full_remote(git: impl AsRef<str>) -> Option<String> {
    let git = git.as_ref();
    if git.len() >= 3 {
        match &git[..3] {
            "gl:" => Some(format!("https://gitlab.com/{}.git", &git[3..])),
            "bb:" => Some(format!("https://bitbucket.org/{}.git", &git[3..])),
            "gh:" => Some(format!("https://github.com/{}.git", &git[3..])),
            _ => None,
        }
    } else {
        None
    }
}

// favorite can be in form of org/repo what should be parsed as github.com
pub fn abbreviated_github(fav: &str) -> Option<String> {
    let org_repo_regex = Regex::new(r"^[a-zA-Z0-9_]+/[a-zA-Z0-9_%-]+$").unwrap();
    org_repo_regex
        .is_match(fav)
        .then(|| format!("https://github.com/{fav}.git"))
}

pub fn local_path(fav: &str) -> Option<PathBuf> {
    let path = PathBuf::from(fav);
    (path.exists() && path.is_dir()).then(|| path)
}

// Template should be cloned with git
pub struct GitUserInput {
    url: String,
    branch: Option<String>,
    identity: Option<PathBuf>,
    _force_init: bool,
}

impl GitUserInput {
    fn new<T1, T2>(
        url: &T1,
        branch: Option<&T2>,
        identity: Option<PathBuf>,
        force_init: bool,
    ) -> Self
    where
        T1: AsRef<str>,
        T2: AsRef<str>,
    {
        Self {
            url: url.as_ref().to_owned(),
            branch: branch.map(|s| s.as_ref().to_owned()),
            identity,
            _force_init: force_init,
        }
    }

    // when git was used as abbreviation but other flags still could be passed
    fn with_git_url_and_args<T1>(url: &T1, args: &GenerateArgs) -> Self
    where
        T1: AsRef<str>,
    {
        Self::new(
            url,
            args.template_path.branch(),
            args.ssh_identity.clone(),
            args.force_git_init,
        )
    }

    pub fn url(&self) -> &str {
        self.url.as_ref()
    }

    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn identity(&self) -> Option<&Path> {
        self.identity.as_deref()
    }
}

// Distinguish between plain copy and clone
pub enum TemplateLocation {
    Git(GitUserInput),
    Path(PathBuf),
}

impl From<GitUserInput> for TemplateLocation {
    fn from(source: GitUserInput) -> Self {
        Self::Git(source)
    }
}

impl<T> From<T> for TemplateLocation
where
    T: AsRef<Path>,
{
    fn from(source: T) -> Self {
        Self::Path(PathBuf::from(source.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_support_bb_gl_gh_abbreviations() {
        assert_eq!(
            &abbreviated_git_url_to_full_remote("gh:foo/bar").unwrap(),
            "https://github.com/foo/bar.git"
        );
        assert_eq!(
            &abbreviated_git_url_to_full_remote("bb:foo/bar").unwrap(),
            "https://bitbucket.org/foo/bar.git"
        );
        assert_eq!(
            &abbreviated_git_url_to_full_remote("gl:foo/bar").unwrap(),
            "https://gitlab.com/foo/bar.git"
        );
        assert!(&abbreviated_git_url_to_full_remote("foo/bar").is_none());
    }

    #[test]
    fn should_appreviation_org_repo_to_github() {
        assert_eq!(
            &abbreviated_github("org/repo").unwrap(),
            "https://github.com/org/repo.git"
        );
        assert!(&abbreviated_github("path/to/a/sth").is_none());
    }
}