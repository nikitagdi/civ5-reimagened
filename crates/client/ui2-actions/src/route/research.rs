//! `research.choose`: pick the technology to research now, or append it to the research queue.

use protocol::Command;
use ui2_model::generated::RefuseWhy;

use crate::intent::Intent;
use crate::route::Ctx;
use crate::validate;

/// Run a `research.choose` intent. The technology must be one the model offers (`research.options`); when
/// queuing it may also be one already in the research queue.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(tech), Some(queue)) = (intent.key("tech"), intent.flag("queue")) else {
        return;
    };
    let Some(research) = validate::research(ctx.model).filter(|r| r.known) else {
        // The core has not answered the research query: nothing can be told to be researchable.
        ctx.refuse(RefuseWhy::Unsupported);
        return;
    };
    let listed =
        research.options.get(tech).is_some() || (queue && research.queue.get(tech).is_some());
    let Some(id) = validate::tech_id(tech).filter(|_| listed) else {
        ctx.refuse(RefuseWhy::TechUnavailable);
        return;
    };
    ctx.send(
        Command::ResearchSet { tech: id, queue },
        "research.choose",
        tech.to_js(),
        None,
    );
}
