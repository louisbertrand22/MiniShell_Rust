//! Gestion des signaux du shell, sur le modèle de bash en interactif:
//! - SIGQUIT (Ctrl-\) est ignoré par le shell;
//! - SIGINT (Ctrl-C) ne tue pas le shell: il lève un drapeau qui fait
//!   abandonner la ligne en cours (reste de la liste, boucles, ...);
//! - les commandes lancées retrouvent le comportement par défaut des deux.
//!
//! Sans contrôle de jobs, le shell et sa commande au premier plan sont dans le
//! même groupe de process: un Ctrl-C au terminal les atteint tous les deux.
//! La commande meurt (comportement par défaut), et le shell, prévenu par le
//! drapeau, n'enchaîne pas sur la suite de la ligne.

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_signal: libc::c_int) {
    // Seule opération faite ici: un store atomique, async-signal-safe.
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Installe les dispositions du shell. À appeler une fois au démarrage.
pub fn install() {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SA_RESTART: un waitpid/read interrompu reprend tout seul, le shell
        // continue donc d'attendre la commande en cours jusqu'à sa mort.
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());

        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }
}

/// Un SIGINT est-il arrivé depuis le dernier `clear()` ?
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Lève le drapeau à la main (ex. Ctrl-C pendant la saisie d'un heredoc,
/// que l'éditeur de ligne reçoit comme une touche et non comme un signal).
pub fn set_interrupted() {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Réarme le drapeau avant chaque nouvelle ligne.
pub fn clear() {
    INTERRUPTED.store(false, Ordering::SeqCst);
}

/// Remet SIGINT/SIGQUIT par défaut, dans un process enfant (forké, avant un
/// éventuel exec). Un signal *ignoré* resterait ignoré à travers exec, d'où
/// ce reset explicite. Async-signal-safe: utilisable dans `pre_exec`.
pub fn reset_for_child() {
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGQUIT, libc::SIG_DFL);
    }
}
