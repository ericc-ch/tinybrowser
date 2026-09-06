use std::error::Error;

use tinybrowser::Page;

fn main() -> Result<(), Box<dyn Error>> {
    let mut page = Page::new();
    page.load_html("<!doctype html><p>page probe</p>");
    if let Some(url) = std::env::args().nth(1) {
        page.goto(&url)?;
        page.run();
    }
    page.eval("setTimeout(() => { globalThis.result = 42; }, 0)")?;
    page.run();
    println!("{}", page.eval("globalThis.result")?);
    Ok(())
}
