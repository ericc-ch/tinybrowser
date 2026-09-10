use std::error::Error;

use tinybrowser::Browser;

fn main() -> Result<(), Box<dyn Error>> {
    let browser = Browser::ephemeral()?;
    let page = browser.handle().create_page()?;
    page.load_html("<!doctype html><p>page probe</p>")?;
    if let Some(url) = std::env::args().nth(1) {
        page.goto(&url)?;
        page.run_until_load()?;
    }
    page.eval("setTimeout(() => { globalThis.result = 42; }, 0)")?;
    page.run()?;
    println!("{}", page.eval("globalThis.result")?);
    Ok(())
}
