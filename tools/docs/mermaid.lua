-- Emit Mermaid code blocks as raw `<pre class="mermaid">` text.
--
-- Mermaid v11 reads `element.innerHTML`, so pandoc's default
-- `<pre class="mermaid"><code>…</code></pre>` makes it see a literal
-- `<code>` tag and report "No diagram type detected".
function CodeBlock(block)
  if not block.classes:includes("mermaid") then
    return nil
  end
  local text = block.text
  text = text:gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;")
  return pandoc.RawBlock("html", '<pre class="mermaid">' .. text .. "</pre>")
end
