#import "/templates/base.typ": post

#show: post.with(
  title: "Hello World",
  date: "2024-01-15",
  author: "Your Name",
  summary: "Your first blog post with Tola and Tailwind CSS",
  tags: ("tutorial", "typst", "tailwind"),
)

Welcome to your first blog post! This template uses Tailwind CSS for styling.

= Text Formatting

You can use *bold*, _italic_, and `inline code`.

#quote[
  Typst is a new markup-based typesetting system that is designed to be as
  powerful as LaTeX while being much easier to learn and use.
]

= Code Blocks

```rust
fn main() {
    println!("Hello from Typst!");
}
```

```python
def greet(name):
    return f"Hello, {name}!"
```

= Lists

Unordered list:
- First item
- Second item
- Third item with `code`

Ordered list:
+ Step one
+ Step two
+ Step three

= Links

Check out these resources:
- #link("https://typst.app/docs")[Typst Documentation]
- #link("https://github.com/tola-ssg/tola-ssg")[Tola on GitHub]
- #link("https://tailwindcss.com/docs")[Tailwind CSS Docs]

= Math

The quadratic formula: $x = (-b plus.minus sqrt(b^2 - 4 a c)) / (2 a)$

A more complex example:

$ sum_(n=0)^infinity x^n / n! = e^x $

= Table

#table(
  columns: 3,
  [Feature], [Status], [Notes],
  [Math], [✓], [Full support],
  [Code], [✓], [Syntax highlighting],
  [Images], [✓], [SVG, PNG, JPEG],
)

---

_Last updated: 2024-01-15_
