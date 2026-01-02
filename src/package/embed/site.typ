// @tola/site:0.0.0 - Site configuration from tola.toml [site]

#let _tola_site_info = sys.inputs.at("__SITE_INFO_KEY__", default: (:))

/// Site metadata from [site.info] section.
/// Access via `info.title`, `info.author`, `info.extra.xxx`, etc.
#let info = (
  title: _tola_site_info.at("title", default: ""),
  author: _tola_site_info.at("author", default: ""),
  email: _tola_site_info.at("email", default: ""),
  description: _tola_site_info.at("description", default: ""),
  url: _tola_site_info.at("url", default: none),
  language: _tola_site_info.at("language", default: "en"),
  copyright: _tola_site_info.at("copyright", default: ""),
  extra: _tola_site_info.at("extra", default: (:)),
)
