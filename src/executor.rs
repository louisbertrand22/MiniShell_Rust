use crate::parser::ast::{Command, LogicalOp, ParamOp, Redirect, Word};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command as SysCommand, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};

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

pub struct Executor;

impl Executor {
    fn set_last_status(code: i32) {
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

    pub fn execute(command: Command) -> Result<(), String> {
        match command {
            Command::Simple(simple_cmd) => {
                let name = Self::expand(&simple_cmd.name);
                let args: Vec<String> = simple_cmd.args.iter().map(Self::expand).collect();

                if let Some(result) = Self::run_builtin(&name, &args, &simple_cmd.redirects) {
                    Self::set_last_status(if result.is_ok() { 0 } else { 1 });
                    return result;
                }

                let (stdin, stdout) =
                    Self::apply_redirects(&simple_cmd.redirects, Stdio::inherit(), Stdio::inherit())?;

                let mut child = match SysCommand::new(&name).args(&args).stdin(stdin).stdout(stdout).spawn() {
                    Ok(child) => child,
                    Err(e) => {
                        Self::set_last_status(127);
                        return Err(format!("Failed to start command '{}': {}", name, e));
                    }
                };

                let status = child
                    .wait()
                    .map_err(|e| format!("Failed to wait for command: {}", e))?;
                Self::set_last_status(status.code().unwrap_or(1));

                if status.success() {
                    Ok(())
                } else {
                    Err(format!("Command exited with non-zero status: {:?}", status.code()))
                }
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
            Command::Pipeline(commands) => Self::execute_pipeline(commands),
            Command::Loop { condition, body, until } => Self::execute_loop(*condition, *body, until),
            Command::For { var, words, body } => Self::execute_for(var, words, *body),
        }
    }

    fn execute_loop(condition: Command, body: Command, until: bool) -> Result<(), String> {
        let mut last_result = Ok(());

        loop {
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

    /// Exécute un builtin comme étage de pipeline en le forkant: l'enfant
    /// branche `stdin_fd`/`stdout_fd` sur ses fds 0/1 puis lance le builtin
    /// directement (sans exec), avant de quitter avec son code de sortie.
    fn fork_builtin(
        name: String,
        args: Vec<String>,
        redirects: Vec<Redirect>,
        stdin_fd: Option<RawFd>,
        stdout_fd: Option<RawFd>,
    ) -> Result<libc::pid_t, String> {
        let pid = unsafe { libc::fork() };

        if pid < 0 {
            return Err(format!("fork: {}", io::Error::last_os_error()));
        }

        if pid != 0 {
            return Ok(pid); // process parent
        }

        // Process enfant: à partir d'ici on ne revient jamais dans execute_pipeline.
        if let Some(fd) = stdin_fd {
            unsafe {
                libc::dup2(fd, 0);
                libc::close(fd);
            }
        }
        if let Some(fd) = stdout_fd {
            unsafe {
                libc::dup2(fd, 1);
                libc::close(fd);
            }
        }

        let code = match Self::run_builtin(&name, &args, &redirects) {
            Some(Ok(())) => 0,
            Some(Err(e)) => {
                eprintln!("{}", e);
                1
            }
            None => unreachable!("fork_builtin appelé avec un nom qui n'est pas un builtin"),
        };

        io::stdout().flush().ok();
        std::process::exit(code);
    }

    fn execute_pipeline(commands: Vec<Command>) -> Result<(), String> {
        let count = commands.len();
        let mut children: Vec<PipelineChild> = Vec::with_capacity(count);
        let mut prev_read: Option<RawFd> = None;

        for (i, command) in commands.into_iter().enumerate() {
            let simple_cmd = match command {
                Command::Simple(simple_cmd) => simple_cmd,
                _ => return Err("Only simple commands are supported in pipelines for now".to_string()),
            };

            let name = Self::expand(&simple_cmd.name);
            let args: Vec<String> = simple_cmd.args.iter().map(Self::expand).collect();

            let (next_read, write_end) = if i + 1 < count {
                let (r, w) = Self::create_pipe()?;
                (Some(r), Some(w))
            } else {
                (None, None)
            };

            if Self::is_builtin(&name) {
                let pid = Self::fork_builtin(name, args, simple_cmd.redirects, prev_read, write_end)?;
                children.push(PipelineChild::Forked(pid));

                // Le parent n'a plus besoin de ses copies: l'enfant en a fait les
                // siennes via dup2. Sans exec côté enfant, FD_CLOEXEC ne joue pas
                // ici, donc c'est à nous de fermer ces fds explicitement.
                if let Some(fd) = prev_read {
                    unsafe { libc::close(fd) };
                }
                if let Some(fd) = write_end {
                    unsafe { libc::close(fd) };
                }
            } else {
                let stdin = match prev_read {
                    Some(fd) => unsafe { Stdio::from_raw_fd(fd) },
                    None => Stdio::inherit(),
                };
                let stdout = match write_end {
                    Some(fd) => unsafe { Stdio::from_raw_fd(fd) },
                    None => Stdio::inherit(),
                };
                // Stdio::from_raw_fd possède le fd: qu'il soit consommé par spawn()
                // ou remplacé ici par apply_redirects, il se ferme tout seul côté
                // parent dès qu'il est droppé — aucune fermeture manuelle requise.
                let (stdin, stdout) = Self::apply_redirects(&simple_cmd.redirects, stdin, stdout)?;

                let child = SysCommand::new(&name)
                    .args(&args)
                    .stdin(stdin)
                    .stdout(stdout)
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
            Self::set_last_status(status.code().unwrap_or(1));
        }

        match last_status {
            Some(status) if status.success() => Ok(()),
            Some(status) => Err(format!("Command exited with non-zero status: {:?}", status.code())),
            None => Ok(()),
        }
    }

    /// Renvoie `Some(résultat)` si `name` est un builtin (exécuté directement dans
    /// le process du shell), ou `None` si ce n'est pas un builtin et doit être
    /// lancé comme un process externe.
    fn run_builtin(name: &str, args: &[String], redirects: &[Redirect]) -> Option<Result<(), String>> {
        match name {
            "cd" => Some(Self::builtin_cd(args, redirects)),
            "pwd" => Some(Self::builtin_pwd(redirects)),
            "export" => Some(Self::builtin_export(args, redirects)),
            "unset" => Some(Self::builtin_unset(args, redirects)),
            "exit" => Some(Self::builtin_exit(args)),
            _ => None,
        }
    }

    /// Ouvre la cible d'une éventuelle redirection de sortie (`>`/`>>`, la dernière
    /// gagnant), ou stdout par défaut. Les builtins qui produisent du texte (pwd,
    /// export) écrivent à travers ce writer plutôt que via `println!` en dur.
    fn resolve_output_target(redirects: &[Redirect]) -> Result<Box<dyn Write>, String> {
        let mut target: Option<Box<dyn Write>> = None;

        for redirect in redirects {
            match redirect {
                Redirect::Output(word) => {
                    let path = Self::expand(word);
                    let file = File::create(&path).map_err(|e| format!("{}: {}", path, e))?;
                    target = Some(Box::new(file));
                }
                Redirect::Append(word) => {
                    let path = Self::expand(word);
                    let file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .map_err(|e| format!("{}: {}", path, e))?;
                    target = Some(Box::new(file));
                }
                Redirect::Input(_) => {} // sans effet pour ces builtins, qui ne lisent pas stdin
            }
        }

        Ok(target.unwrap_or_else(|| Box::new(io::stdout())))
    }

    fn builtin_cd(args: &[String], redirects: &[Redirect]) -> Result<(), String> {
        Self::resolve_output_target(redirects)?; // ouvre/tronque une éventuelle cible, comme un vrai shell

        let target = match args.first() {
            Some(path) => path.clone(),
            None => std::env::var("HOME").map_err(|_| "cd: HOME not set".to_string())?,
        };

        std::env::set_current_dir(&target).map_err(|e| format!("cd: {}: {}", target, e))
    }

    fn builtin_pwd(redirects: &[Redirect]) -> Result<(), String> {
        let cwd = std::env::current_dir().map_err(|e| format!("pwd: {}", e))?;
        let mut out = Self::resolve_output_target(redirects)?;
        writeln!(out, "{}", cwd.display()).map_err(|e| format!("pwd: {}", e))
    }

    fn builtin_export(args: &[String], redirects: &[Redirect]) -> Result<(), String> {
        if args.is_empty() {
            let mut vars: Vec<(String, String)> = std::env::vars().collect();
            vars.sort_by(|a, b| a.0.cmp(&b.0));

            let mut out = Self::resolve_output_target(redirects)?;
            for (key, value) in vars {
                writeln!(out, "export {}={}", key, value).map_err(|e| format!("export: {}", e))?;
            }
            return Ok(());
        }

        Self::resolve_output_target(redirects)?;

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

    fn builtin_unset(args: &[String], redirects: &[Redirect]) -> Result<(), String> {
        Self::resolve_output_target(redirects)?;

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

    /// Applique les redirections sur des Stdio de base (héritées ou issues d'un pipe).
    /// Une redirection explicite prend toujours le pas sur le comportement par défaut,
    /// et la dernière redirection d'un même type gagne (comme dans un vrai shell).
    fn apply_redirects(
        redirects: &[Redirect],
        mut stdin: Stdio,
        mut stdout: Stdio,
    ) -> Result<(Stdio, Stdio), String> {
        for redirect in redirects {
            match redirect {
                Redirect::Output(word) => {
                    let path = Self::expand(word);
                    let file = File::create(&path)
                        .map_err(|e| format!("{}: {}", path, e))?;
                    stdout = Stdio::from(file);
                }
                Redirect::Append(word) => {
                    let path = Self::expand(word);
                    let file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .map_err(|e| format!("{}: {}", path, e))?;
                    stdout = Stdio::from(file);
                }
                Redirect::Input(word) => {
                    let path = Self::expand(word);
                    let file = File::open(&path)
                        .map_err(|e| format!("{}: {}", path, e))?;
                    stdin = Stdio::from(file);
                }
            }
        }

        Ok((stdin, stdout))
    }
}