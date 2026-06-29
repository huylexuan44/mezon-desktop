use scap::Target;

#[derive(Debug, Clone)]
pub enum PickedScreen {
    Target(Target),
    #[cfg(target_os = "linux")]
    LinuxPortal,
}

pub(crate) fn scap_target_for_pick(pick: PickedScreen) -> Result<Option<Target>, String> {
    match pick {
        PickedScreen::Target(target) => Ok(Some(resolve_target(&target)?)),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal => Ok(None),
    }
}

fn resolve_target(target: &Target) -> Result<Target, String> {
    let targets = scap::get_all_targets().map_err(|e| e.to_string())?;
    match target {
        Target::Window(window) => targets
            .into_iter()
            .find(|candidate| match candidate {
                Target::Window(candidate) => candidate.id == window.id,
                _ => false,
            })
            .ok_or_else(|| format!("window \"{}\" is no longer available", window.title)),
        Target::Display(display) => targets
            .into_iter()
            .find(|candidate| match candidate {
                Target::Display(candidate) => candidate.id == display.id,
                _ => false,
            })
            .ok_or_else(|| format!("display \"{}\" is no longer available", display.title)),
    }
}
