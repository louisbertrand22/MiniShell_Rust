use crate::signals;
use crate::parser::ast::{Command, LogicalOp, ParamOp, Redirect, RedirectKind, Word};
use std::fs::OpenOptions;
use std::io::{self, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command as SysCommand, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// Code de sortie de la dernière commande exécutée, exposé via `$?`.
static LAST_STATUS: AtomicI32 = AtomicI32::new(0);

/// Un builtin exécuté comme étage d'un pipeline tourne dans un vrai process
/// forké (comme un vrai shell), pour que `cd`/`export`/`unset`/`exit` n'y
/// affectent qu'une copie de l'environnement/du cwd, jamais le shell parent.
enum PipelineChild {
    Process(std::process::Child),
    Forked(libc::pid_t),
}

impl PipelineChild {
    fn wait(self) -> Result<ExitStatus, String> {
        match self {
            PipelineChild::Process(mut child) => child
                .wait()
                .map_err(|e| format!("Failed to wait for command: {}", e)),
            PipelineChild::Forked(pid) => {
                let mut status: i32 = 0;
                let ret = unsafe { libc::waitpid(pid, &mut status, 0) };
                if ret < 0 {
                    return Err(format!(
                        "Failed to wait for pid {}: {}",
                        pid,
                        io::Error::last_os_error()
                    ));
                }
                Ok(ExitStatus::from_raw(status))
            }
        }
    }
}

/// Plus petit fd utilisé pour les fichiers ouverts par les redirections et
/// les sauvegardes de fds: les garder hors de 0..=9 évite qu'un `dup2` sur un
/// fd cible écrase un fichier qu'une action suivante doit encore lire
/// (ex. `4>a 3>b`, où `a` aurait sinon pu être ouvert sur le fd 4).
const HIGH_FD: RawFd = 10;

/// Une étape de redirection, appliquée dans l'ordre d'écriture: l'ordre compte
/// (`>f 2>&1` envoie les deux flux dans `f`, `2>&1 >f` seulement stdout).
#[derive(Debug, Clone, Copy)]
enum FdAction {
    /// `dup2(src, target)`
    Dup { src: RawFd, target: RawFd },
    /// `close(target)`
    Close(RawFd),
}

impl FdAction {
    fn target(self) -> RawFd {
        match self {
            FdAction::Dup { target, .. } | FdAction::Close(target) => target,
        }
    }
}

/// Redirections résolues: fichiers ouverts (gardés vivants jusqu'à ce que
/// l'enfant les ait récupérés) et actions à appliquer sur les fds.
struct PreparedRedirects {
    actions: Vec<FdAction>,
    _files: Vec<OwnedFd>,
}

pub struct Executor;

impl Executor {
    pub fn set_last_status(code: i32) {
        LAST_STATUS.store(code, Ordering::Relaxed);
    }

    fn last_status() -> i32 {
        LAST_STATUS.load(Ordering::Relaxed)
    }

    /// Résout un nom de variable en sa valeur: `?` et `$` sont des paramètres
    /// spéciaux (dernier code de sortie, PID du shell), le reste vient de
    /// l'environnement du process (vide si non définie).
    fn expand_variable(name: &str) -> String {
        match name {
            "?" => Self::last_status().to_string(),
            "$" => std::process::id().to_string(),
            _ => std::env::var(name).unwrap_or_default(),
        }
    }

    /// Résout un `Word` en la chaîne effective à utiliser.
    fn expand(word: &Word) -> String {
        match word {
            Word::Literal(s) => s.clone(),
            Word::Variable(name) => Self::expand_variable(name),
            Word::Concat(parts) => parts.iter().map(Self::expand).collect(),
            Word::Parameter { name, op, default } => {
                let current = Self::expand_variable(name);
                match op {
                    None => current,
                    Some(ParamOp::UseDefault) => {
                        if current.is_empty() { Self::expand(default) } else { current }
                    }
                    Some(ParamOp::AssignDefault) => {
                        if current.is_empty() {
                            let value = Self::expand(default);
                            // Safe: ce shell est mono-thread, aucun autre code ne
                            // lit/écrit l'environnement en parallèle.
                            unsafe { std::env::set_var(name, &value) };
                            value
                        } else {
                            current
                        }
                    }
                    Some(ParamOp::UseAlternative) => {
                        if current.is_empty() { String::new() } else { Self::expand(default) }
                    }
                }
            }
        }
    }

    /// Code de sortie façon shell: le code du process, ou 128 + numéro du
    /// signal qui l'a tué (130 pour SIGINT, 131 pour SIGQUIT, ...).
    fn status_code(status: ExitStatus) -> i32 {
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    }

    /// Traduit le statut d'une commande en résultat. Une mort par SIGINT est
    /// silencieuse (l'utilisateur vient de taper Ctrl-C, comme dans bash).
    fn status_result(status: ExitStatus) -> Result<(), String> {
        match (status.code(), status.signal()) {
            _ if status.success() => Ok(()),
            (Some(code), _) => Err(format!("Command exited with non-zero status: {code}")),
            (None, Some(libc::SIGINT)) => Err(String::new()),
            (None, Some(signal)) => Err(format!("Command terminated by signal {signal}")),
            (None, None) => Err("Command terminated abnormally".to_string()),
        }
    }

    pub fn execute(command: Command) -> Result<(), String> {
        // Après un Ctrl-C, plus rien de la ligne en cours ne s'exécute.
        if signals::interrupted() {
            Self::set_last_status(130);
            return Err(String::new());
        }

        match command {
            Command::Simple(simple_cmd) => {
                let name = Self::expand(&simple_cmd.name);
                let args: Vec<String> = simple_cmd.args.iter().map(Self::expand).collect();
                let prepared = Self::prepare_redirects(&simple_cmd.redirects).inspect_err(|_| Self::set_last_status(1))?;

                if Self::is_builtin(&name) {
                    // L'erreur est affichée tant que les redirections sont actives,
                    // pour que `cd nowhere 2>/dev/null` reste silencieux.
                    let result = Self::with_redirects_in_shell(&prepared, || {
                        Self::run_builtin(&name, &args).inspect_err(|e| Self::report_error(e))
                    })
                    .inspect_err(|_| Self::set_last_status(1))?;
                    Self::set_last_status(if result.is_ok() { 0 } else { 1 });
                    return result.map_err(|_| String::new());
                }

                let mut command = SysCommand::new(&name);
                command.args(&args);
                Self::prepare_child(&mut command, &prepared);

                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(e) => {
                        Self::set_last_status(127);
                        let message = format!("Failed to start command '{}': {}", name, e);
                        // Comme un vrai shell, le message suit les redirections de la
                        // commande (`cmd_inconnue 2>/dev/null` n'affiche rien).
                        if Self::with_redirects_in_shell(&prepared, || Self::report_error(&message)).is_err() {
                            Self::report_error(&message);
                        }
                        return Err(String::new());
                    }
                };
                drop(prepared); // l'enfant a ses copies: on libère les nôtres

                let status = child
                    .wait()
                    .map_err(|e| format!("Failed to wait for command: {}", e))?;
                Self::set_last_status(Self::status_code(status));

                Self::status_result(status)
            }
            Command::BinaryOp { left, op, right } => {
                let left_res = Self::execute(*left);

                match op {
                    LogicalOp::And => {
                        if left_res.is_ok() {
                            Self::execute(*right)
                        } else {
                            left_res
                        }
                    }
                    LogicalOp::Or => {
                        if left_res.is_err() {
                            Self::execute(*right)
                        } else {
                            left_res
                        }
                    }
                }
            }
            Command::If { condition, then_part, else_part } => {
                if Self::execute(*condition).is_ok() {
                    Self::execute(*then_part)
                } else if let Some(else_cmd) = else_part {
                    Self::execute(*else_cmd)
                } else {
                    Ok(())
                }
            }
            Command::Sequence(commands) => {
                let mut last_result: Result<(), String> = Ok(());
                for command in commands {
                    // Une commande qui échoue n'arrête pas la liste: son erreur est
                    // affichée tout de suite, seule celle de la dernière remonte.
                    if let Err(e) = &last_result
                        && !e.is_empty()
                    {
                        Self::report_error(e);
                    }
                    last_result = Self::execute(command);
                }
                last_result
            }
            Command::Pipeline(commands) => Self::execute_pipeline(commands),
            Command::Loop { condition, body, until } => Self::execute_loop(*condition, *body, until),
            Command::For { var, words, body } => Self::execute_for(var, words, *body),
            Command::Redirected { command, redirects } => {
                let prepared = Self::prepare_redirects(&redirects).inspect_err(|_| Self::set_last_status(1))?;
                // Appliquées sur les fds du shell lui-même: les commandes externes
                // lancées dans la structure en héritent, les builtins aussi.
                Self::with_redirects_in_shell(&prepared, || Self::execute(*command))
                    .inspect_err(|_| Self::set_last_status(1))?
            }
            Command::Not(command) => {
                // Le message d'échec de la commande niée n'a plus lieu d'être:
                // son échec est justement le cas attendu.
                if Self::execute(*command).is_ok() {
                    Self::set_last_status(1);
                    Err(String::new())
                } else {
                    Self::set_last_status(0);
                    Ok(())
                }
            }
        }
    }

    fn execute_loop(condition: Command, body: Command, until: bool) -> Result<(), String> {
        let mut last_result = Ok(());

        loop {
            if signals::interrupted() {
                return Err(String::new());
            }
            let condition_ok = Self::execute(condition.clone()).is_ok();
            let should_continue = if until { !condition_ok } else { condition_ok };
            if !should_continue {
                break;
            }
            last_result = Self::execute(body.clone());
        }

        last_result
    }

    fn execute_for(var: String, words: Vec<Word>, body: Command) -> Result<(), String> {
        let mut last_result = Ok(());

        for word in words {
            if signals::interrupted() {
                return Err(String::new());
            }
            let value = Self::expand(&word);
            // Exposée comme variable d'environnement: c'est ce qui permet à `$var`
            // (une autre `Word::Variable`) de la relire dans le corps de la boucle.
            // Safe: ce shell est mono-thread, aucun autre code ne lit/écrit l'environnement en parallèle.
            unsafe {
                std::env::set_var(&var, &value);
            }
            last_result = Self::execute(body.clone());
        }

        last_result
    }

    fn is_builtin(name: &str) -> bool {
        matches!(name, "cd" | "pwd" | "export" | "unset" | "exit")
    }

    /// Crée un pipe avec `FD_CLOEXEC` sur les deux extrémités, pour qu'un
    /// process externe exec'd ne récupère jamais par erreur les fds des
    /// *autres* étages du pipeline (seuls ceux explicitement dup2'd sur 0/1
    /// dans l'enfant survivent à l'exec, puisque dup2 ne recopie pas ce flag).
    fn create_pipe() -> Result<(RawFd, RawFd), String> {
        let mut fds = [0 as RawFd; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(format!("pipe: {}", io::Error::last_os_error()));
        }
        for fd in fds {
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
        Ok((fds[0], fds[1]))
    }

    /// Exécute un étage de pipeline dans une copie forkée du shell (sans
    /// exec): l'enfant branche `stdin_fd`/`stdout_fd` sur ses fds 0/1, ferme
    /// `extra_fd` (le bout de lecture du pipe suivant, qui ne le concerne pas),
    /// lance `run`, puis quitte avec le code renvoyé. Sert aux builtins et aux
    /// structures (`if`, boucles, ...), dont les effets (cd, export, ...)
    /// restent ainsi confinés à l'enfant, comme le subshell d'un vrai shell.
    fn fork_stage(
        stdin_fd: Option<RawFd>,
        stdout_fd: Option<RawFd>,
        extra_fd: Option<RawFd>,
        run: impl FnOnce() -> i32,
    ) -> Result<libc::pid_t, String> {
        // Sinon un texte encore en buffer serait écrit deux fois (parent + enfant).
        io::stdout().flush().ok();
        let pid = unsafe { libc::fork() };

        if pid < 0 {
            return Err(format!("fork: {}", io::Error::last_os_error()));
        }

        if pid != 0 {
            return Ok(pid); // process parent
        }

        // Process enfant: à partir d'ici on ne revient jamais dans execute_pipeline.
        signals::reset_for_child();
        for (fd, target) in [(stdin_fd, 0), (stdout_fd, 1)] {
            if let Some(fd) = fd {
                unsafe {
                    libc::dup2(fd, target);
                    libc::close(fd);
                }
            }
        }
        if let Some(fd) = extra_fd {
            unsafe { libc::close(fd) };
        }

        let code = run();
        io::stdout().flush().ok();
        std::process::exit(code);
    }

    /// Corps d'un builtin forké: ses redirections, puis le builtin lui-même.
    fn run_forked_builtin(name: &str, args: &[String], prepared: &PreparedRedirects) -> i32 {
        let result = Self::apply_fd_actions(&prepared.actions)
            .map_err(|e| format!("{name}: {e}"))
            .and_then(|()| Self::run_builtin(name, args));
        match result {
            Ok(()) => 0,
            Err(e) => {
                Self::report_error(&e);
                1
            }
        }
    }

    /// Corps d'une structure forkée: l'executor normal, dans l'enfant. Le code
    /// de sortie est celui de sa dernière commande, comme pour `$?`.
    fn run_forked_command(command: Command) -> i32 {
        match Self::execute(command) {
            Ok(()) => 0,
            Err(e) => {
                if !e.is_empty() {
                    Self::report_error(&e);
                }
                // Une erreur sans statut enregistré (ex. erreur de redirection
                // déjà comptée) ne doit jamais devenir un succès.
                match Self::last_status() {
                    0 => 1,
                    code => code,
                }
            }
        }
    }

    /// Ferme les copies parent des fds passés à un étage forké: l'enfant en a
    /// fait les siennes via dup2, et sans exec, FD_CLOEXEC ne joue pas ici.
    /// Indispensable pour qu'un lecteur voie la fin de fichier du pipe.
    fn close_fds(fds: [Option<RawFd>; 2]) {
        for fd in fds.into_iter().flatten() {
            unsafe { libc::close(fd) };
        }
    }

    fn execute_pipeline(commands: Vec<Command>) -> Result<(), String> {
        let count = commands.len();
        let mut children: Vec<PipelineChild> = Vec::with_capacity(count);
        let mut prev_read: Option<RawFd> = None;

        for (i, command) in commands.into_iter().enumerate() {
            let (next_read, write_end) = if i + 1 < count {
                let (r, w) = Self::create_pipe()?;
                (Some(r), Some(w))
            } else {
                (None, None)
            };

            let simple_cmd = match command {
                Command::Simple(simple_cmd) => simple_cmd,
                compound => {
                    // Structure (if, boucle, ...): tout l'executor tourne dans l'enfant.
                    let pid = Self::fork_stage(prev_read, write_end, next_read, || Self::run_forked_command(compound))?;
                    children.push(PipelineChild::Forked(pid));
                    Self::close_fds([prev_read, write_end]);
                    prev_read = next_read;
                    continue;
                }
            };

            let name = Self::expand(&simple_cmd.name);
            let args: Vec<String> = simple_cmd.args.iter().map(Self::expand).collect();

            let prepared = Self::prepare_redirects(&simple_cmd.redirects).inspect_err(|_| {
                Self::close_fds([prev_read, write_end]);
                Self::close_fds([next_read, None]);
            })?;

            if Self::is_builtin(&name) {
                let pid = Self::fork_stage(prev_read, write_end, next_read, || {
                    Self::run_forked_builtin(&name, &args, &prepared)
                })?;
                children.push(PipelineChild::Forked(pid));
                Self::close_fds([prev_read, write_end]);
            } else {
                let stdin = match prev_read {
                    Some(fd) => unsafe { Stdio::from_raw_fd(fd) },
                    None => Stdio::inherit(),
                };
                let stdout = match write_end {
                    Some(fd) => unsafe { Stdio::from_raw_fd(fd) },
                    None => Stdio::inherit(),
                };
                // Stdio::from_raw_fd possède le fd: consommé par spawn(), il se
                // ferme tout seul côté parent — aucune fermeture manuelle requise.
                // Les pipes sont branchés avant pre_exec, donc les redirections
                // explicites de l'étage passent par-dessus.
                let mut command = SysCommand::new(&name);
                command.args(&args).stdin(stdin).stdout(stdout);
                Self::prepare_child(&mut command, &prepared);

                let child = command
                    .spawn()
                    .map_err(|e| format!("Failed to start command '{}': {}", name, e))?;

                children.push(PipelineChild::Process(child));
            }

            prev_read = next_read;
        }

        let mut last_status = None;
        for child in children {
            last_status = Some(child.wait()?);
        }

        if let Some(status) = &last_status {
            Self::set_last_status(Self::status_code(*status));
        }

        last_status.map_or(Ok(()), Self::status_result)
    }

    /// Lance le builtin `name` dans le process courant, qui doit déjà avoir
    /// ses redirections appliquées (sortie sur stdout, erreurs renvoyées).
    fn run_builtin(name: &str, args: &[String]) -> Result<(), String> {
        match name {
            "cd" => Self::builtin_cd(args),
            "pwd" => Self::builtin_pwd(),
            "export" => Self::builtin_export(args),
            "unset" => Self::builtin_unset(args),
            "exit" => Self::builtin_exit(args),
            _ => unreachable!("run_builtin appelé avec un nom qui n'est pas un builtin: {}", name),
        }
    }

    /// Affiche une erreur sur stderr (tel qu'il est au moment de l'appel, donc
    /// en suivant d'éventuelles redirections actives).
    fn report_error(message: &str) {
        eprintln!("Execution Error: {}", message);
    }

    fn builtin_cd(args: &[String]) -> Result<(), String> {
        let target = match args.first() {
            Some(path) => path.clone(),
            None => std::env::var("HOME").map_err(|_| "cd: HOME not set".to_string())?,
        };

        std::env::set_current_dir(&target).map_err(|e| format!("cd: {}: {}", target, e))
    }

    fn builtin_pwd() -> Result<(), String> {
        let cwd = std::env::current_dir().map_err(|e| format!("pwd: {}", e))?;
        writeln!(io::stdout(), "{}", cwd.display()).map_err(|e| format!("pwd: {}", e))
    }

    fn builtin_export(args: &[String]) -> Result<(), String> {
        if args.is_empty() {
            let mut vars: Vec<(String, String)> = std::env::vars().collect();
            vars.sort_by(|a, b| a.0.cmp(&b.0));

            let mut out = io::stdout().lock();
            for (key, value) in vars {
                writeln!(out, "export {}={}", key, value).map_err(|e| format!("export: {}", e))?;
            }
            return Ok(());
        }

        for arg in args {
            match arg.split_once('=') {
                // Safe: ce shell est mono-thread, aucun autre code ne lit/écrit l'environnement en parallèle.
                Some((key, value)) => unsafe { std::env::set_var(key, value) },
                None => {
                    if std::env::var(arg).is_err() {
                        unsafe { std::env::set_var(arg, "") };
                    }
                }
            }
        }

        Ok(())
    }

    fn builtin_unset(args: &[String]) -> Result<(), String> {
        for arg in args {
            // Safe: ce shell est mono-thread, aucun autre code ne lit/écrit l'environnement en parallèle.
            unsafe { std::env::remove_var(arg) };
        }

        Ok(())
    }

    fn builtin_exit(args: &[String]) -> Result<(), String> {
        let code = match args.first() {
            None => 0,
            Some(arg) => match arg.parse::<i32>() {
                Ok(n) => n,
                Err(_) => {
                    eprintln!("exit: {}: numeric argument required", arg);
                    255
                }
            },
        };

        io::stdout().flush().ok();
        std::process::exit(code);
    }

    /// Déplace un fd fraîchement ouvert vers un numéro >= `HIGH_FD` (avec
    /// `FD_CLOEXEC`), pour qu'aucun `dup2` vers une cible basse ne l'écrase.
    fn move_high(fd: OwnedFd) -> Result<OwnedFd, String> {
        let high = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, HIGH_FD) };
        if high < 0 {
            return Err(format!("fcntl: {}", io::Error::last_os_error()));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(high) })
    }

    fn open_file(path: &str, options: &OpenOptions) -> Result<OwnedFd, String> {
        let file = options.open(path).map_err(|e| format!("{}: {}", path, e))?;
        Self::move_high(file.into())
    }

    /// Écrit le corps d'un heredoc dans un fichier temporaire déjà supprimé
    /// (il vit tant qu'un fd pointe dessus), rembobiné, prêt à être lu.
    /// Contrairement à un pipe, aucun risque de blocage sur un gros corps.
    fn heredoc_fd(body: &str) -> Result<OwnedFd, String> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "minishell-heredoc-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let to_err = |e: io::Error| format!("heredoc: {}", e);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(to_err)?;
        std::fs::remove_file(&path).ok();
        file.write_all(body.as_bytes()).map_err(to_err)?;
        file.seek(SeekFrom::Start(0)).map_err(to_err)?;
        Self::move_high(file.into())
    }

    /// Résout les redirections dans le shell (expansion des cibles, ouverture
    /// des fichiers), pour que les erreurs (fichier introuvable, ...) soient
    /// signalées avant tout fork/spawn.
    fn prepare_redirects(redirects: &[Redirect]) -> Result<PreparedRedirects, String> {
        let mut actions = Vec::new();
        let mut files = Vec::new();

        for redirect in redirects {
            let target = redirect.fd;
            let opened = match &redirect.kind {
                RedirectKind::Output(word) => {
                    Self::open_file(&Self::expand(word), OpenOptions::new().write(true).create(true).truncate(true))?
                }
                RedirectKind::Append(word) => {
                    Self::open_file(&Self::expand(word), OpenOptions::new().create(true).append(true))?
                }
                RedirectKind::Input(word) => Self::open_file(&Self::expand(word), OpenOptions::new().read(true))?,
                RedirectKind::ReadWrite(word) => {
                    Self::open_file(&Self::expand(word), OpenOptions::new().read(true).write(true).create(true))?
                }
                RedirectKind::Heredoc(body) => Self::heredoc_fd(&Self::expand(body))?,
                RedirectKind::Dup(word) => {
                    let source = Self::expand(word);
                    if source == "-" {
                        actions.push(FdAction::Close(target));
                    } else {
                        let src = source
                            .parse::<RawFd>()
                            .map_err(|_| format!("{}: ambiguous redirect", source))?;
                        actions.push(FdAction::Dup { src, target });
                    }
                    continue;
                }
            };
            actions.push(FdAction::Dup { src: opened.as_raw_fd(), target });
            files.push(opened);
        }

        Ok(PreparedRedirects { actions, _files: files })
    }

    /// Applique les actions sur les fds du process courant, dans l'ordre.
    /// N'utilise que des appels async-signal-safe: appelable depuis `pre_exec`.
    fn apply_fd_actions(actions: &[FdAction]) -> io::Result<()> {
        for action in actions {
            match *action {
                FdAction::Dup { src, target } if src == target => {
                    // dup2 sur lui-même est un no-op qui garderait FD_CLOEXEC:
                    // il faut le retirer à la main pour que le fd survive à l'exec.
                    let flags = unsafe { libc::fcntl(src, libc::F_GETFD) };
                    if flags < 0 || unsafe { libc::fcntl(src, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                FdAction::Dup { src, target } => {
                    if unsafe { libc::dup2(src, target) } < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                FdAction::Close(target) => unsafe {
                    libc::close(target);
                },
            }
        }
        Ok(())
    }

    /// Prépare l'enfant juste avant son exec (après le branchement des pipes
    /// stdin/stdout fait par `Command`): signaux par défaut, puis redirections.
    fn prepare_child(command: &mut SysCommand, prepared: &PreparedRedirects) {
        let actions = prepared.actions.clone();
        // Safe: la closure ne fait que signal/dup2/close/fcntl, async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                signals::reset_for_child();
                Self::apply_fd_actions(&actions)
            });
        }
    }

    /// Exécute `f` dans le shell lui-même avec les redirections appliquées,
    /// puis remet chaque fd touché dans son état d'origine (sauvegardé sur un
    /// fd haut, ou refermé s'il n'était pas ouvert). Utilisé pour les builtins,
    /// qui doivent tourner dans le shell pour affecter son état.
    fn with_redirects_in_shell<T>(prepared: &PreparedRedirects, f: impl FnOnce() -> T) -> Result<T, String> {
        if prepared.actions.is_empty() {
            return Ok(f());
        }

        let mut saved: Vec<(RawFd, Option<RawFd>)> = Vec::new();
        for action in &prepared.actions {
            let target = action.target();
            if !saved.iter().any(|(fd, _)| *fd == target) {
                let copy = unsafe { libc::fcntl(target, libc::F_DUPFD_CLOEXEC, HIGH_FD) };
                saved.push((target, (copy >= 0).then_some(copy)));
            }
        }

        io::stdout().flush().ok();
        let applied = Self::apply_fd_actions(&prepared.actions);
        let result = applied.is_ok().then(f);
        io::stdout().flush().ok(); // vide le buffer vers la cible redirigée avant de restaurer

        for (target, copy) in saved.into_iter().rev() {
            unsafe {
                match copy {
                    Some(copy) => {
                        libc::dup2(copy, target);
                        libc::close(copy);
                    }
                    None => {
                        libc::close(target);
                    }
                }
            }
        }

        applied.map_err(|e| e.to_string())?;
        Ok(result.expect("f runs whenever the redirections applied"))
    }
}
