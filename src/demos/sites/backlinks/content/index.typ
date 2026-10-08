#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Body links become backlinks"))
#title()
The navigation points at every page. Only links written in labeled article bodies become backlinks.

#link("https://example.test/demo/topic/")[An absolute URL to Topic] remains external, even with the site's origin.
This home page has no site-local body link to Topic, so it is absent from Topic's incoming list.
