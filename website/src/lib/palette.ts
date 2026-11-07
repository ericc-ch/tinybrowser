export type Palette = Record<string, string>

export function parsePalettes(css: string): { light: Palette; dark: Palette } {
  const scope = /:root\s*\{([^}]*)\}/.exec(css)
  if (!scope) throw new Error("Could not find the :root palette block")

  const light: Palette = {}
  const dark: Palette = {}
  const tokens = /(--[a-z0-9-]+):\s*light-dark\(\s*(#[0-9a-f]{6})\s*,\s*(#[0-9a-f]{6})\s*\)/gi

  for (const [, token, lightValue, darkValue] of scope[1].matchAll(tokens)) {
    light[token] = lightValue
    dark[token] = darkValue
  }

  if (Object.keys(light).length === 0) {
    throw new Error("No light-dark() tokens found in the :root palette block")
  }

  return { light, dark }
}
