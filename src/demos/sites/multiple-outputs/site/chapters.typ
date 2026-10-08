#let chapter-list(chapters) = html.ul[
  #for chapter in chapters [#html.li[*#chapter.title*: #chapter.text]]
]
#let chapter-table(chapters) = html.table[
  #html.thead[#html.tr[#html.th[Chapter]#html.th[Purpose]]]
  #html.tbody[
    #for chapter in chapters [#html.tr[#html.td[#chapter.title]#html.td[#chapter.text]]]
  ]
]
