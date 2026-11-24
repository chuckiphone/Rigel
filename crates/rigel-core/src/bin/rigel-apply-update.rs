//! The deferred-replace helper as a standalone process.
//!
//! The desktop app spawns a second copy of itself with `--rigel-apply-update <plan>` and nothing
//! else. This binary runs the same `rigel_core::update::apply` code so the swap can be exercised
//! headlessly, with a throwaway copy of an executable instead of the installed app.

fn main() {
    let plan = match rigel_core::update::apply::helper_request(std::env::args_os().skip(1)) {
        Some(plan) => plan,
        None => {
            eprintln!(
                "rigel-apply-update: pass {} <plan.json>",
                rigel_core::update::HELPER_FLAG
            );
            std::process::exit(2);
        }
    };
    match rigel_core::update::apply::run_helper(&plan) {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("rigel-apply-update: {error}");
            std::process::exit(1);
        }
    }
}
