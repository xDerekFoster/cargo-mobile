#![forbid(unsafe_code)]

use cargo_mobile::{
    apple::{
        config::{Config, Metadata},
        device::{Device, RunError},
        ios_deploy,
        target::{ArchiveError, BuildError, CheckError, CompileLibError, ExportError, Target},
        NAME,
    },
    config::{
        metadata::{self, Metadata as OmniMetadata},
        Config as OmniConfig, LoadOrGenError,
    },
    define_device_prompt,
    device::PromptError,
    env::{Env, Error as EnvError},
    init, opts, os,
    target::{call_for_targets_with_fallback, TargetInvalid, TargetTrait as _},
    util::{
        self,
        cli::{self, Exec, GlobalFlags, Report, Reportable, TextWrapper},
        prompt,
    },
};
use std::{collections::HashMap, ffi::OsStr, path::PathBuf};
use structopt::{clap::AppSettings, StructOpt};

#[derive(Debug, StructOpt)]
#[structopt(bin_name = cli::bin_name(NAME), global_settings = cli::GLOBAL_SETTINGS, settings = cli::SETTINGS)]
pub struct Input {
    #[structopt(flatten)]
    flags: GlobalFlags,
    #[structopt(subcommand)]
    command: Command,
}

fn macos_from_platform(platform: &str) -> bool {
    platform == "macOS"
}

fn profile_from_configuration(configuration: &str) -> opts::Profile {
    if configuration == "release" {
        opts::Profile::Release
    } else {
        opts::Profile::Debug
    }
}

#[derive(Debug, StructOpt)]
pub enum Command {
    #[structopt(
        name = "init",
        about = "Creates a new project in the current working directory"
    )]
    Init {
        #[structopt(flatten)]
        skip_dev_tools: cli::SkipDevTools,
        #[structopt(flatten)]
        reinstall_deps: cli::ReinstallDeps,
        #[structopt(
            long = "open",
            help = "Open in Xcode",
            parse(from_flag = opts::OpenInEditor::from_bool),
        )]
        open_in_editor: opts::OpenInEditor,
        #[structopt(long = "submodule-commit", help = "Template pack commit to checkout")]
        submodule_commit: Option<String>,
    },
    #[structopt(name = "open", about = "Open project in Xcode")]
    Open,
    #[structopt(name = "check", about = "Checks if code compiles for target(s)")]
    Check {
        #[structopt(name = "targets", default_value = Target::DEFAULT_KEY, possible_values = Target::name_list())]
        targets: Vec<String>,
    },
    #[structopt(name = "build", about = "Builds static libraries for target(s)")]
    Build {
        #[structopt(name = "targets", default_value = Target::DEFAULT_KEY, possible_values = Target::name_list())]
        targets: Vec<String>,
        #[structopt(flatten)]
        profile: cli::Profile,
    },
    #[structopt(name = "archive", about = "Builds and archives for targets(s)")]
    Archive {
        #[structopt(name = "targets", default_value = Target::DEFAULT_KEY, possible_values = Target::name_list())]
        targets: Vec<String>,
        #[structopt(flatten)]
        profile: cli::Profile,
    },
    #[structopt(name = "run", about = "Deploys IPA to connected device")]
    Run {
        #[structopt(flatten)]
        profile: cli::Profile,
    },
    #[structopt(name = "list", about = "Lists connected devices")]
    List,
    #[structopt(
        name = "xcode-script",
        about = "Compiles static lib (should only be called by Xcode!)",
        setting = AppSettings::Hidden
    )]
    XcodeScript {
        #[structopt(
            long = "platform",
            help = "Value of `PLATFORM_DISPLAY_NAME` env var",
            parse(from_str = macos_from_platform),
        )]
        macos: bool,
        #[structopt(long = "sdk-root", help = "Value of `SDKROOT` env var")]
        sdk_root: PathBuf,
        #[structopt(
            long = "configuration",
            help = "Value of `CONFIGURATION` env var",
            parse(from_str = profile_from_configuration),
        )]
        profile: opts::Profile,
        #[structopt(
            long = "force-color",
            help = "Value of `FORCE_COLOR` env var",
            parse(from_flag = opts::ForceColor::from_bool),
        )]
        force_color: opts::ForceColor,
        #[structopt(
            name = "ARCHS",
            help = "Value of `ARCHS` env var",
            index = 1,
            required = true
        )]
        arches: Vec<String>,
    },
}

#[derive(Debug)]
pub enum Error {
    EnvInitFailed(EnvError),
    DevicePromptFailed(PromptError<ios_deploy::DeviceListError>),
    TargetInvalid(TargetInvalid),
    ConfigFailed(LoadOrGenError),
    MetadataFailed(metadata::Error),
    ProjectDirAbsent { project_dir: PathBuf },
    InitFailed(init::Error),
    OpenFailed(bossy::Error),
    CheckFailed(CheckError),
    BuildFailed(BuildError),
    ArchiveFailed(ArchiveError),
    ExportFailed(ExportError),
    RunFailed(RunError),
    ListFailed(ios_deploy::DeviceListError),
    NoHomeDir(util::NoHomeDir),
    CargoEnvFailed(bossy::Error),
    SdkRootInvalid { sdk_root: PathBuf },
    IncludeDirInvalid { include_dir: PathBuf },
    MacosSdkRootInvalid { macos_sdk_root: PathBuf },
    ArchInvalid { arch: String },
    CompileLibFailed(CompileLibError),
}

impl Reportable for Error {
    fn report(&self) -> Report {
        match self {
            Self::EnvInitFailed(err) => err.report(),
            Self::DevicePromptFailed(err) => err.report(),
            Self::TargetInvalid(err) => Report::error("Specified target was invalid", err),
            Self::ConfigFailed(err) => err.report(),
            Self::MetadataFailed(err) => err.report(),
            Self::ProjectDirAbsent { project_dir } => Report::action_request(
                "Please run `cargo mobile init` and try again!",
                format!("Xcode project directory {:?} doesn't exist.", project_dir),
            ),
            Self::InitFailed(err) => err.report(),
            Self::OpenFailed(err) => Report::error("Failed to open project in Xcode", err),
            Self::CheckFailed(err) => err.report(),
            Self::BuildFailed(err) => err.report(),
            Self::ArchiveFailed(err) => err.report(),
            Self::ExportFailed(err) => err.report(),
            Self::RunFailed(err) => err.report(),
            Self::ListFailed(err) => err.report(),
            Self::NoHomeDir(err) => Report::error("Failed to load cargo env profile", err),
            Self::CargoEnvFailed(err) => Report::error("Failed to load cargo env profile", err),
            Self::SdkRootInvalid { sdk_root } => Report::error(
                "SDK root provided by Xcode was invalid",
                format!("{:?} doesn't exist or isn't a directory", sdk_root),
            ),
            Self::IncludeDirInvalid { include_dir } => Report::error(
                "Include dir was invalid",
                format!("{:?} doesn't exist or isn't a directory", include_dir),
            ),
            Self::MacosSdkRootInvalid { macos_sdk_root } => Report::error(
                "macOS SDK root was invalid",
                format!("{:?} doesn't exist or isn't a directory", macos_sdk_root),
            ),
            Self::ArchInvalid { arch } => Report::error(
                "Arch specified by Xcode was invalid",
                format!("{:?} isn't a known arch", arch),
            ),
            Self::CompileLibFailed(err) => err.report(),
        }
    }
}

impl Exec for Input {
    type Report = Error;

    fn global_flags(&self) -> GlobalFlags {
        self.flags
    }

    fn exec(self, wrapper: &TextWrapper) -> Result<(), Self::Report> {
        define_device_prompt!(ios_deploy::device_list, ios_deploy::DeviceListError, iOS);
        fn detect_target_ok<'a>(env: &Env) -> Option<&'a Target<'a>> {
            device_prompt(env).map(|device| device.target()).ok()
        }

        fn with_config(
            non_interactive: opts::NonInteractive,
            wrapper: &TextWrapper,
            f: impl FnOnce(&Config) -> Result<(), Error>,
        ) -> Result<(), Error> {
            let (config, _origin) = OmniConfig::load_or_gen(".", non_interactive, wrapper)
                .map_err(Error::ConfigFailed)?;
            f(config.apple())
        }

        fn with_config_and_metadata(
            non_interactive: opts::NonInteractive,
            wrapper: &TextWrapper,
            f: impl FnOnce(&Config, &Metadata) -> Result<(), Error>,
        ) -> Result<(), Error> {
            with_config(non_interactive, wrapper, |config| {
                let metadata =
                    OmniMetadata::load(&config.app().root_dir()).map_err(Error::MetadataFailed)?;
                f(config, &metadata.apple)
            })
        }

        fn ensure_init(config: &Config) -> Result<(), Error> {
            if !config.project_dir_exists() {
                Err(Error::ProjectDirAbsent {
                    project_dir: config.project_dir(),
                })
            } else {
                Ok(())
            }
        }

        fn open_in_xcode(config: &Config) -> Result<(), Error> {
            os::open_file_with("Xcode", config.project_dir()).map_err(Error::OpenFailed)
        }

        let Self {
            flags:
                GlobalFlags {
                    noise_level,
                    non_interactive,
                },
            command,
        } = self;
        let env = Env::new().map_err(Error::EnvInitFailed)?;
        match command {
            Command::Init {
                skip_dev_tools: cli::SkipDevTools { skip_dev_tools },
                reinstall_deps: cli::ReinstallDeps { reinstall_deps },
                open_in_editor,
                submodule_commit,
            } => {
                let config = init::exec(
                    wrapper,
                    non_interactive,
                    skip_dev_tools,
                    reinstall_deps,
                    Default::default(),
                    Some(vec!["apple".into()]),
                    None,
                    submodule_commit,
                    ".",
                )
                .map_err(Error::InitFailed)?;
                if open_in_editor.yes() {
                    open_in_xcode(config.apple())
                } else {
                    Ok(())
                }
            }
            Command::Open => with_config(non_interactive, wrapper, |config| {
                ensure_init(config)?;
                open_in_xcode(config)
            }),
            Command::Check { targets } => {
                with_config_and_metadata(non_interactive, wrapper, |config, metadata| {
                    call_for_targets_with_fallback(
                        targets.iter(),
                        &detect_target_ok,
                        &env,
                        |target: &Target| {
                            target
                                .check(config, metadata, &env, noise_level)
                                .map_err(Error::CheckFailed)
                        },
                    )
                    .map_err(Error::TargetInvalid)?
                })
            }
            Command::Build {
                targets,
                profile: cli::Profile { profile },
            } => with_config(non_interactive, wrapper, |config| {
                ensure_init(config)?;
                call_for_targets_with_fallback(
                    targets.iter(),
                    &detect_target_ok,
                    &env,
                    |target: &Target| {
                        target
                            .build(config, &env, noise_level, profile)
                            .map_err(Error::BuildFailed)
                    },
                )
                .map_err(Error::TargetInvalid)?
            }),
            Command::Archive {
                targets,
                profile: cli::Profile { profile },
            } => with_config(non_interactive, wrapper, |config| {
                ensure_init(config)?;
                call_for_targets_with_fallback(
                    targets.iter(),
                    &detect_target_ok,
                    &env,
                    |target: &Target| {
                        target
                            .build(config, &env, noise_level, profile)
                            .map_err(Error::BuildFailed)?;
                        target
                            .archive(config, &env, noise_level, profile)
                            .map_err(Error::ArchiveFailed)
                    },
                )
                .map_err(Error::TargetInvalid)?
            }),
            Command::Run {
                profile: cli::Profile { profile },
            } => with_config(non_interactive, wrapper, |config| {
                ensure_init(config)?;
                device_prompt(&env)
                    .map_err(Error::DevicePromptFailed)?
                    .run(config, &env, wrapper, noise_level, non_interactive, profile)
                    .map_err(Error::RunFailed)
            }),
            Command::List => ios_deploy::device_list(&env)
                .map_err(Error::ListFailed)
                .map(|device_list| {
                    prompt::list_display_only(device_list.iter(), device_list.len());
                }),
            Command::XcodeScript {
                macos,
                sdk_root,
                profile,
                force_color,
                arches,
            } => with_config_and_metadata(non_interactive, wrapper, |config, metadata| {
                // The `PATH` env var Xcode gives us is missing any additions
                // made by the user's profile, so we'll manually add cargo's
                // `PATH`.
                let env = env.prepend_to_path(
                    util::home_dir()
                        .map_err(Error::NoHomeDir)?
                        .join(".cargo/bin"),
                );

                if !sdk_root.is_dir() {
                    return Err(Error::SdkRootInvalid { sdk_root });
                }
                let include_dir = sdk_root.join("usr/include");
                if !include_dir.is_dir() {
                    return Err(Error::IncludeDirInvalid { include_dir });
                }

                let mut host_env = HashMap::<&str, &OsStr>::new();

                // Host flags that are used by build scripts
                let macos_isysroot = {
                    let macos_sdk_root =
                        sdk_root.join("../../../../MacOSX.platform/Developer/SDKs/MacOSX.sdk");
                    if !macos_sdk_root.is_dir() {
                        return Err(Error::MacosSdkRootInvalid { macos_sdk_root });
                    }
                    format!("-isysroot {}", macos_sdk_root.display())
                };
                host_env.insert("MAC_FLAGS", macos_isysroot.as_ref());
                host_env.insert("CFLAGS_x86_64_apple_darwin", macos_isysroot.as_ref());
                host_env.insert("CXXFLAGS_x86_64_apple_darwin", macos_isysroot.as_ref());

                host_env.insert(
                    "OBJC_INCLUDE_PATH_x86_64_apple_darwin",
                    include_dir.as_os_str(),
                );

                host_env.insert("RUST_BACKTRACE", "1".as_ref());

                let macos_target = Target::macos();

                let isysroot = format!("-isysroot {}", sdk_root.display());

                for arch in arches {
                    // Set target-specific flags
                    let triple = match arch.as_str() {
                        "arm64" => "aarch64_apple_ios",
                        "x86_64" => "x86_64_apple_ios",
                        _ => return Err(Error::ArchInvalid { arch }),
                    };
                    let cflags = format!("CFLAGS_{}", triple);
                    let cxxflags = format!("CFLAGS_{}", triple);
                    let objc_include_path = format!("OBJC_INCLUDE_PATH_{}", triple);
                    let mut target_env = host_env.clone();
                    target_env.insert(cflags.as_ref(), isysroot.as_ref());
                    target_env.insert(cxxflags.as_ref(), isysroot.as_ref());
                    target_env.insert(objc_include_path.as_ref(), include_dir.as_ref());

                    let target = if macos {
                        &macos_target
                    } else {
                        Target::for_arch(&arch).ok_or_else(|| Error::ArchInvalid {
                            arch: arch.to_owned(),
                        })?
                    };
                    target
                        .compile_lib(
                            config,
                            metadata,
                            noise_level,
                            force_color,
                            profile,
                            &env,
                            target_env,
                        )
                        .map_err(Error::CompileLibFailed)?;
                }
                Ok(())
            }),
        }
    }
}

fn main() {
    cli::exec::<Input>(NAME)
}
