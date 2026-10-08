#import "page.typ": page
#let not-found() = page("404.html", "Page not found")[
  #title()
  This demo has no page at this address.
]
