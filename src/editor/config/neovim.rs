//! Neovim 0.11+ language-server registration as a Lua snippet.

use super::EditorDirectory;

pub(super) fn snippet(directory: &EditorDirectory) -> String {
    let command = directory.server_command();
    let mut executable = vec![command.command.to_owned()];
    executable.extend(command.arguments);
    let command = lua_json(&serde_json::json!(executable));
    let root = lua_json(&serde_json::json!(directory.root()));
    // A configuration answers Tola's own keys in its file; a directory without one is answered
    // through the Typst documents it holds.
    let (configuration, options) = match directory.configuration_file() {
        Some(configuration_file) => (
            format!(
                "local selected_config = vim.fn.fnamemodify({}, ':p')\n",
                lua_json(&serde_json::json!(configuration_file))
            ),
            r#"-- The site's configuration answers Tola's key completion and hover.
  filetypes = { 'typst', 'toml' },
  root_dir = function(buffer, on_dir)
    local name = vim.api.nvim_buf_get_name(buffer)
    if vim.fs.relpath(root, name) and vim.fn.filereadable(selected_config) == 1 then on_dir(root) end
  end,"#,
        ),
        None => (
            String::new(),
            r#"-- Without a site configuration Tola answers for the Typst documents in root.
  filetypes = { 'typst' },
  root_dir = function(buffer, on_dir)
    local name = vim.api.nvim_buf_get_name(buffer)
    if vim.fs.relpath(root, name) then on_dir(root) end
  end,"#,
        ),
    };
    format!(
        r#"-- Neovim 0.11+. Tola answers the site's sources and the ordinary Typst beside them.
local root = vim.fs.normalize(vim.fn.fnamemodify({root}, ':p'))
{configuration}
-- Definitions into a builtin package arrive as `tola-package:` documents; their source is
-- fetched from the server and shown in a read-only buffer.
local function show_package_source(client, uri, range)
  client:request('tola/source', {{ uri = uri }}, function(err, source)
    if err then vim.notify(err.message, vim.log.levels.ERROR); return end
    if not source or type(source.text) ~= 'string' then return end
    local buffer = vim.uri_to_bufnr(uri)
    vim.bo[buffer].modifiable = true
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.split(source.text, '\n', {{ plain = true }}))
    vim.bo[buffer].buftype = 'nofile'
    vim.bo[buffer].bufhidden = 'hide'
    vim.bo[buffer].swapfile = false
    vim.bo[buffer].filetype = 'typst'
    vim.bo[buffer].modified = false
    vim.bo[buffer].readonly = true
    vim.bo[buffer].modifiable = false
    vim.lsp.util.show_document({{ uri = uri, range = range }}, client.offset_encoding, {{ focus = true }})
  end)
end

local function define(client, buffer)
  local params = vim.lsp.util.make_position_params(0, client.offset_encoding)
  client:request('textDocument/definition', params, function(err, result, context)
    if err then vim.notify(err.message, vim.log.levels.ERROR); return end
    local location = vim.islist(result) and result[1] or result
    if not location then return end
    local uri = location.targetUri or location.uri
    local range = location.targetSelectionRange or location.range
    if uri:match('^tola%-package:') then
      show_package_source(client, uri, range)
    else
      vim.lsp.util.show_document(location, client.offset_encoding, {{ focus = true }})
    end
  end, buffer)
end

local function attach(client, buffer)
  vim.keymap.set('n', 'K', vim.lsp.buf.hover, {{ buffer = buffer, desc = 'Tola hover' }})
  vim.keymap.set('n', 'gd', function() define(client, buffer) end, {{ buffer = buffer, desc = 'Tola definition' }})
  vim.keymap.set('i', '<C-k>', vim.lsp.buf.signature_help, {{ buffer = buffer, desc = 'Tola signature' }})
  vim.lsp.completion.enable(true, client.id, buffer, {{ autotrigger = true }})
end

local options = {{
  cmd = {command},
{options}
  on_attach = attach,
}}
-- Preserve an existing custom Tola executable/options as well.
vim.lsp.config('tola-lsp', vim.tbl_deep_extend('keep', vim.lsp.config['tola-lsp'] or {{}}, options))
vim.lsp.enable({{ 'tola-lsp' }})
"#
    )
}

fn lua_json(value: &serde_json::Value) -> String {
    let json = serde_json::to_string(value).expect("editor settings are serializable");
    let mut delimiter = String::new();
    while json.contains(&format!("]{delimiter}]")) || json.ends_with(&format!("]{delimiter}")) {
        delimiter.push('=');
    }
    format!("vim.json.decode([{delimiter}[{json}]{delimiter}])")
}

#[cfg(test)]
mod tests {
    use super::super::tests::editor_directory;
    use super::*;

    #[test]
    fn snippet_declares_both_filetypes() {
        let snippet = snippet(&editor_directory("."));
        assert!(
            snippet.contains("filetypes = { 'typst', 'toml' }"),
            "{snippet}"
        );
    }

    #[test]
    fn json_content_cannot_close_lua_string() {
        let value = serde_json::json!({ "path": "workspace]]and]=]" });
        let encoded = lua_json(&value);
        let json = encoded
            .strip_prefix("vim.json.decode([==[")
            .unwrap()
            .strip_suffix("]==])")
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(json).unwrap(),
            value
        );
    }
}
