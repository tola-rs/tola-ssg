#import "@tola/source:0.0.0": tola-meta
#import "@tola/address:0.0.0": output-to-url
#tola-meta((title: "Guide contents"))
#title()

= Start <start>
This labeled heading has a stable anchor.

== Detail
This unlabeled heading gets an anchor because the TOC links to its native location.

=== Deep detail
The TOC stops at declared level two.

#heading(level: 2, outlined: false)[Aside]
This heading is visible in the body but excluded from the TOC.

Visit the #link(output-to-url("other/index.html"))[other document] to see its own contents.
