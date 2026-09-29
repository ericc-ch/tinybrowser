import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

test("hosted Sauce Demo checkout", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  try {
    const page = browser.contexts()[0].pages()[0];
    await page.goto("https://www.saucedemo.com/");
    await expect(page.locator("#user-name")).toBeVisible();

    await page.locator("#user-name").fill("standard_user");
    await page.locator("#password").fill("secret_sauce");
    await page.locator("#login-button").click();
    await expect(page.locator(".inventory_list")).toBeVisible();
    await expect(page.locator(".inventory_item img").first()).toBeVisible();
    await expect.poll(() => page.evaluate(() =>
      document.querySelector<HTMLImageElement>(".inventory_item img")?.naturalWidth ?? 0,
    )).toBeGreaterThan(0);
    await expect.poll(() => page.evaluate(() => {
      const image = document.querySelector<HTMLImageElement>(".inventory_item img");
      const frame = document.querySelector<HTMLElement>(".inventory_item_img");
      if (!image || !frame || !image.naturalHeight) return Infinity;
      const box = image.getBoundingClientRect();
      const height = frame.getBoundingClientRect().height;
      return Math.abs(box.height - height)
        + Math.abs(box.width - height * image.naturalWidth / image.naturalHeight);
    })).toBeLessThan(2);

    await page.locator(".product_sort_container").selectOption("lohi");
    const products = page.locator(".inventory_item");
    const firstName = await products.nth(0).locator(".inventory_item_name").textContent();
    const secondName = await products.nth(1).locator(".inventory_item_name").textContent();
    await products.nth(0).getByRole("button", { name: "Add to cart" }).click();
    await products.nth(1).getByRole("button", { name: "Add to cart" }).click();

    await page.locator(".shopping_cart_link").click();
    await expect(page.locator(".cart_item")).toHaveCount(2);
    await expect(page.locator(".cart_item").nth(0)).toContainText(firstName ?? "");
    await expect(page.locator(".cart_item").nth(1)).toContainText(secondName ?? "");
    await page.locator("#checkout").click();
    await page.locator("#first-name").fill("Tiny");
    await page.locator("#last-name").fill("Browser");
    await page.locator("#postal-code").fill("12345");
    await page.locator("#continue").click();
    await expect(page.locator(".cart_item")).toHaveCount(2);
    await page.locator("#finish").click();
    await expect(page.locator(".complete-header")).toHaveText("Thank you for your order!");
  } finally {
    await browser.close();
  }
});
