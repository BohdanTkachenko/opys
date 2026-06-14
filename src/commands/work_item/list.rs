use crate::cli::ListFormat;
use crate::error::Result;
use crate::work_item::WorkItem;
use crate::Ctx;

fn matches(w: &WorkItem, feature: Option<&str>, status: Option<&str>) -> bool {
    if let Some(feat) = feature {
        if !w.feature_refs().iter().any(|f| f == feat) {
            return false;
        }
    }
    if let Some(status) = status {
        if w.status() != Some(status) {
            return false;
        }
    }
    true
}

pub fn run(
    ctx: &Ctx,
    feature: Option<&str>,
    status: Option<&str>,
    format: ListFormat,
) -> Result<()> {
    let prj = ctx.open()?;
    prj.require_wi_cfg()?;
    let (items, _) = prj.load_work_items();
    for w in items.iter().filter(|w| matches(w, feature, status)) {
        match format {
            ListFormat::Ids => println!("{}", w.id().unwrap_or("")),
            ListFormat::Paths => println!("{}", w.path.display()),
            ListFormat::Table => {
                let feats = w.feature_refs().join(", ");
                println!(
                    "{}  [{}]  ({})  {}",
                    w.id().unwrap_or(""),
                    w.status().unwrap_or(""),
                    feats,
                    w.title
                );
            }
        }
    }
    Ok(())
}
