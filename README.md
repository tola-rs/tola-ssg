# tola-ssg

## Introduction

`tola`: A static site generator for Typst-based blogs.  
It handles the most tedious tasks unrelated to Typst itself.  

e.g.,  
- Automatically extract embedded SVG images for smaller size and faster loading
- Slugify paths and fragments for posts
- Watch for changes and recompile automatically
- Local server for previewing the generated site
- Eliminate repetitive commands that users don't need to worry about
- Built-in Tailwind CSS support, out of the box
- Deploy the generated site to GitHub Pages (other providers are planned)
- Provide template files with a small kernel, so users can easily customize their own site (planned)
- RSS 2.0 support

## Philosophy

The philosophy of `tola`:  
**Keep your focus on the content itself.**  

`tola` helps you get closer to that goal!  
- Provide a lightweight and minimal abstraction layer that allows users to work without being locked into a rigid framework.
- If you can accomplish something more easily with Typst in your posts, then use it. (e.g., [TODO](todo))
- Keep the core simple and maintainable, and enjoy the pure joy of writing.

## Note

- This documentation is still a work in progress. More details and tutorials coming soon.
- The experience of Typst/HTML is fundamentally different from Typst/PDF. `tola` is designed for users who want to use Typst as a replacement for both LaTeX and Markdown for personal blog, and who are open to embracing html/css/javascript for convenience, simplicity, and aesthetics.
- `tola` aims to offer maximum flexibility and freedom — almost everything can be easily customized by yourself. It eliminates boilerplate and repetitive code, providing as many conveniences as possible so you can focus purely on your content.

## Examples

[My blog](https://kawayww.com):

![home](/screenshots/home.avif)  
![figure1](/screenshots/figure1.avif)  
![figure2](/screenshots/figure2.avif)  

## Installation

- `cargo install tola`
- Install the binary from the [release page](https://github.com/KawaYww/tola/releases).
- For Nix users, a `flake.nix` already exists in the repo root, and you can use the binary cache at [tola.cachix.org](https://tola.cachix.org):

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    tola = {
      url = "github:kawayww/tola";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # ...
    # ...
  }
  # ...
  # ...
}
```

```nix
# configuration.nix
{ config, pkgs, inputs, ... }:

{
  nix.settings = {
    substituters = [
      "https://tola.cachix.org"
      # ...
      # ...
    ];
    trusted-public-keys = [
      "tola.cachix.org-1:5hMwVpNfWcOlq0MyYuU9QOoNr6bRcRzXBMt/Ua2NbgA="
      # ...
      # ...
    ];
    environment.systemPackages = with pkgs; [
      # ...
      # ...
    ] ++ [
      inputs.tola.packages.${pkgs.system}.default
    ];
  }
}
```

## Usage

- `tola -h`:  

```text
A static site generator for typst-based blog

Usage: tola [OPTIONS] <COMMAND>

Commands:
  init    Init a template site
  serve   Serve the site. Rebuild and reload on change automatically
  build   Deletes the output directory if there is one and rebuilds the site
  deploy  Deletes the output directory if there is one and rebuilds the site
  help    Print this message or the help of the given subcommand(s)

Options:
  -r, --root <ROOT>          root directory path
  -o, --output <OUTPUT>      Output directory path related to `root`
  -c, --content <CONTENT>    Content directory path related to `root`
  -a, --assets <ASSETS>      Assets directory path related to `root`
  -C, --config <CONFIG>      Config file path related to `root` [default: tola.toml]
  -m, --minify <MINIFY>      Minify the html content [possible values: true, false]
  -t, --tailwind <TAILWIND>  enable tailwindcss support [possible values: true, false]
  -h, --help                 Print help
  -V, --version              Print version
```

Your project should follow the directory structure below:

```text
.
├── assets
│   ├── fonts
│   ├── iconfonts
│   ├── images
│   ├── scripts
│   ├── styles
├── content
│   ├── posts/
│   ├── categories/
│   ├── index.typ
│   ├── programming.typ
├── templates
│   └── tola-conf.typ
├── utils
│   └── main.typ
└── utils
    └── main.typ
```

Files under the `content/` directory are mapped to their respective routes:  
e.g., `content/posts/examples/aaa.typ` -> `http://127.0.0.1:5277/posts/examples/aaa`  
(`content/index.typ` will be specially compiled into `http://127.0.0.1:5277/index.html`)  

```text
http://127.0.0.1:5277:
├── assets
│   ├── fonts
│   ├── iconfonts
│   ├── images
│   ├── scripts
│   └─ styles
├── posts/
├── categories/
└── index.html
```