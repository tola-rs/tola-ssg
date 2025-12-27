# Tola Starter Template

A minimal blog template built with [Tola](https://github.com/tola-ssg/tola-ssg) and [Tailwind CSS](https://tailwindcss.com).

## Prerequisites

- [tola](https://github.com/tola-ssg/tola-ssg) - Install via `cargo install tola`
- [tailwindcss](https://tailwindcss.com/docs/installation) - Install via `npm install -g tailwindcss` or use npx

> **Note**: This template uses Tailwind CSS v4. Make sure you have the latest version installed.

## Quick Start

```sh
# Start the development server
tola serve

# Build for production
tola build
```

## Project Structure

```
.
├── tola.toml              # Tola configuration
├── content/               # Typst source files
│   ├── index.typ          # Homepage
│   └── posts/             # Blog posts
├── templates/             # Typst templates
│   └── base.typ           # Base template with styles
├── assets/
│   └── styles/
│       └── tailwind.css   # Tailwind input file
└── utils/                 # Shared Typst utilities
```

## Configuration

Edit `tola.toml` to customize:

- **Site metadata**: title, author, description
- **Build options**: minify, RSS, sitemap
- **Tailwind**: already enabled in this template
- **Deploy**: GitHub Pages settings

## Writing Posts

Create new posts in `content/posts/`:

```typst
#import "/templates/base.typ": post-template

#show: post-template.with(
  title: "My Post Title",
  date: "2024-01-15",
  tags: ("blog", "typst"),
)

Your content here...
```

## Customization

- **Colors**: Edit the `@theme` section in `assets/styles/tailwind.css`
- **Layout**: Modify `templates/base.typ`
- **Components**: Add reusable functions in `utils/`

## Deploy

```sh
# Deploy to GitHub Pages
tola deploy
```

Make sure to set your repository URL in `tola.toml` under `[deploy.github]`.

## Learn More

- [Tola Documentation](https://github.com/tola-ssg/tola-ssg)
- [Typst Documentation](https://typst.app/docs)
- [Tailwind CSS Documentation](https://tailwindcss.com/docs)
