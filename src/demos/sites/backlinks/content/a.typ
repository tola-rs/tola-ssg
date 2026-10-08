#import "@tola/source:0.0.0": tola-meta
#import "@tola/address:0.0.0": output-to-url
#tola-meta((title: "Alpha"))
#title()
A #link(<topic-page>)[native page link] and a
#link(output-to-url("topic/index.html"))[URL link] both point at Topic.
There are two occurrences, but the list chooses one row per linking document.
