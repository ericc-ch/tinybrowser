use std::error::Error;

use tinybrowser::Browser;

fn main() -> Result<(), Box<dyn Error>> {
    let browser = Browser::ephemeral()?;
    let tab = browser.handle().create_tab()?;
    tab.load_html("<!doctype html><p>tab probe</p>")?;
    if let Some(url) = std::env::args().nth(1) {
        tab.goto(&url)?;
        tab.run_until_load()?;
    }
    tab.eval("setTimeout(() => { globalThis.result = 42; }, 0)")?;
    tab.run()?;
    println!("{}", tab.eval("globalThis.result")?);
    Ok(())
}
