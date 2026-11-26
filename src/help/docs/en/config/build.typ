#show: config(
  summary: [Configure the Typst entry file, content directory, publish directory, and hook concurrency.],
  related: ("[assets]", "[hooks]", "@tola/source"),
)

The entry file compiles one Typst Bundle: it selects sources, renders each page with `#document(...)`, and maps them to output paths under `publish-dir`.

Tola excludes the entry file from content discovery; keep shared templates and helper modules outside `content-dir`. All outputs—documents, #ref("[assets]"), and #ref("[[hooks.generate-outputs]]")—are validated together before replacing `publish-dir`. If compilation, validation, or a pre-publish hook fails or is cancelled, the previously published directory stays intact.

#example(title: [Default build layout])[
  ```toml
  [build]
  entry = "site.typ"
  content-dir = "content"
  publish-dir = "public"
  ```
]

#example(title: [Publish every discovered source])[
  ```typst
  #import "@tola/address:0.0.0": join-route, output-of
  #import "@tola/source:0.0.0": all-sources

  #for source in all-sources() {
    document(output-of(join-route(source.route-segments)), format: "html")[
      #include source.file
    ]
  }
  ```
]

#key("entry")[
  Root Typst file relative to the site root, or `{ module = "alias" }` to use an entry declared by a configured module.
]

#key("content-dir")[
  Directory scanned by `all-sources()`, relative to the site root. Must stay outside `.tola`.
]

#key("publish-dir")[
  Destination directory written by `tola build`, relative to the site root. Choose an empty directory or the previous publish directory—a successful build replaces its entire contents.
]

#key("par-hooks")[
  Allow hooks that share the same stage, site or module instance, and `order` to run concurrently. Set to `false` to execute one hook at a time while preserving batch order.
]

#key("hook-jobs")[
  Maximum number of hooks running concurrently across all modules and stages in one session. Accepts `"auto"` (sampled host parallelism at startup) or a positive integer. Does not restrict threads spawned internally by a script.
]
