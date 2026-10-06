import { expect } from '@std/expect'
import { describe, test } from '@std/testing/bdd'

import { CEILING, NAME_SHAPES, refusal, rustTests, TITLE_SHAPES, typescriptTitles } from './test-names.ts'

/** One name the audit must refuse, filed under the shape it proves. */
const refused = [
  { shape: 'article or scaffold prefix', name: 'a_route_opens' },
  { shape: 'article or scaffold prefix', name: 'an_error_stops_the_build' },
  { shape: 'article or scaffold prefix', name: 'the_site_compiles' },
  { shape: 'article or scaffold prefix', name: 'test_route_opens' },
  { shape: 'article or scaffold prefix', name: 'it_recovers_the_revision' },
  { shape: 'article or scaffold prefix', name: 'should_keep_the_root' },
  { shape: 'article or scaffold prefix', name: 'when_the_file_changes_the_cache_is_dropped' },
  { shape: 'case position', name: 'b_second_case' },
  { shape: 'case position', name: 'c_last_attempt' },
  { shape: 'connective chain', name: 'adds_a_field_and_updates_the_index_and_seals_it' },
  { shape: 'connective chain', name: 'keeps_the_root_or_names_the_owner_or_fails' },
  { shape: 'connective prose', name: 'routes_answer_or_a_fallback_is_named' },
  { shape: 'connective prose', name: 'transparency_forces_png_or_a_background' },
  { shape: 'connective prose', name: 'reloads_the_page_but_the_cache_stays' },
  { shape: 'dangling article', name: 'keeps_the_root_the' },
  { shape: 'indefinite article', name: 'absent_variant_is_a_miss' },
  { shape: 'indefinite article', name: 'parse_errors_reply_without_an_id' },
  { shape: 'bare verdict', name: 'render_works' },
  { shape: 'bare verdict', name: 'build_succeeds' },
  { shape: 'bare verdict', name: 'parses_is_ok' },
  { shape: 'bare verdict', name: 'hook_passes' },
  { shape: 'bare verdict', name: 'output_functions' },
  { shape: 'hedge', name: 'cache_reuse_works_correctly' },
  { shape: 'hedge', name: 'resolver_behaves_properly' },
  { shape: 'hedge', name: 'symlink_ends_up_fine' },
  { shape: 'hedge', name: 'route_stays_valid' },
  { shape: 'generic verb', name: 'command_handles' },
  { shape: 'generic verb', name: 'rebuild_behaves' },
  { shape: 'generic verb', name: 'does_the_right_thing' },
  { shape: 'single word', name: 'parses' },
  { shape: 'order marker', name: 'route_is_resolved_2' },
  { shape: 'order marker', name: 'writes_the_index_part_3' },
  { shape: 'placeholder word', name: 'fixture_route_opens' },
  { shape: 'placeholder word', name: 'probe_routeless_output' },
  { shape: 'ceiling', name: Array.from({ length: 12 }, () => 'segment').join('_') },
]

/** One case title the audit must refuse. */
const refusedTitles = [
  { shape: 'scaffold prefix', title: 'test the init flow' },
  { shape: 'scaffold prefix', title: 'it handles a restart' },
  { shape: 'scaffold prefix', title: 'Should keep the root' },
  { shape: 'bare verdict', title: 'vendor works' },
  { shape: 'bare verdict', title: 'the rebuild succeeds' },
  { shape: 'hedge', title: 'reload behaves correctly' },
  { shape: 'hedge', title: 'the copy is written properly' },
  { shape: 'placeholder word', title: 'the tar probe reads the header' },
  { shape: 'single word', title: 'rendering' },
]

/** Names worth keeping: a real behaviour, or a domain word a coarse rule would swallow. */
const kept = [
  'mismatched_snapshot_root_fails',
  'writes_partial_output_then_fails',
  'given_path_keeps_its_spelling',
  'first_pass_window',
  'second_axis',
  'b_tree_splits_at_the_middle',
  'k_means_clusters_points',
  'analysis_reports_every_export',
  'missing_package_names_each_directory',
  'query_and_fragment_are_refused',
  'head_and_tail_survive_the_bound',
  'base_href_decides_internal_or_external',
  'device_handles_are_rejected',
  'custom_404_page_is_served',
  'ipv6_rules_expand_compressed_groups',
  'math_helpers_render_svg_with_alt',
  'content_entry_keeps_helpers_visible',
  'attach_keeps_the_typed_failure',
  'evidence_stales_when_the_file_changes',
  'ancestor_bounds_the_read_region',
  'directory_file_and_missing_routes_serve_exact',
]

/** Titles worth keeping: a sentence names what the case proves, not a shape above. */
const keptTitles = [
  'in-place edit preserves node identity',
  'the rebuild indicator waits out the reveal delay',
  'Selected config edits clear diagnostics',
  'source copies retain nonignored edits and symlinks, but exclude ignored files',
  'a failed preview keeps the published bytes',
  'a workspace without a configuration answers about its documents',
  'a workspace without a configuration completes names',
]

/** The Rust declarations whose reading the audit must not get wrong. */
const rustSource = `// #[test]
// fn commented_out() {}
/* #[test]
   fn inside_a_block_comment() {} */
const TEXT: &str = "#[test] fn inside_a_string() {}";
const RAW: &str = r"#[test] fn inside_a_raw_string() {}";
#[cfg(test)]
mod tests {
    #[test]
    fn writes_the_declared_output() {}

    #[tokio::test]
    async fn publishes_the_revision() {}

    #[ignore = "subprocess helper"]
    #[test]
    fn reads_the_closed_pipe() {}

    #[test]
    // the comment above the declaration
    fn keeps_the_root() {}

    #[test] fn names_the_missing_package() {}

    #[test]
    #[should_panic]
    fn rejects_a_foreign_owner() {}

    fn helper_without_a_test() {}
}
macro_rules! cases {
    ($name:ident) => {
        #[test]
        fn $name() {}
    };
}
`

/** The suite calls whose titles the audit must read, and the ones it must not read as a case. */
const suiteSource = `test('in-place edit preserves node identity', async () => {})

test(
  'reload keeps the revision',
  async () => {},
)

for (const scope of scopes) {
  test(\`\${scope} survives icon configuration reloads\`, async () => {})
}

test.describe('documents', () => {
  test('head metadata patches without reload', async () => {})
})

export async function check(name: string, assertion: () => Promise<void>) {}

await check("Selected config edits clear diagnostics", async () => {})

const dynamic = 'a title'
test(dynamic, async () => {})

check()
`

describe('test names', () => {
  test('every refused shape refuses its example', () => {
    for (const { shape, name } of refused) {
      const rule = NAME_SHAPES.find((candidate) => candidate.shape === shape)
      expect(refusal(name, NAME_SHAPES), name).toBe(`${rule?.message}: ${name}`)
    }
    for (const { shape, title } of refusedTitles) {
      const rule = TITLE_SHAPES.find((candidate) => candidate.shape === shape)
      expect(refusal(title, TITLE_SHAPES), title).toBe(`${rule?.message}: ${title}`)
    }
  })

  test('domain names remain admissible', () => {
    for (const name of kept) {
      expect(refusal(name, NAME_SHAPES), name).toBeUndefined()
      expect(name.length).toBeLessThanOrEqual(CEILING)
    }
    for (const title of keptTitles) {
      expect(refusal(title, TITLE_SHAPES), title).toBeUndefined()
    }
  })

  test('rust declarations are read with their lines', () => {
    const { tests, unreadable } = rustTests(rustSource, 'src/example.rs')
    expect(tests.map((declared) => declared.name)).toEqual([
      'writes_the_declared_output',
      'publishes_the_revision',
      'reads_the_closed_pipe',
      'keeps_the_root',
      'names_the_missing_package',
      'rejects_a_foreign_owner',
    ])
    expect(tests.map((declared) => declared.line)).toEqual([10, 13, 17, 21, 23, 27])
    expect(unreadable).toEqual([
      { path: 'src/example.rs', line: 33, detail: 'declares a test this audit cannot read a function from' },
    ])
  })

  test('suite titles are read from literals only', () => {
    const { titles, unreadable } = typescriptTitles(suiteSource, 'e2e/example.spec.ts')
    expect(titles.map((declared) => declared.name)).toEqual([
      'in-place edit preserves node identity',
      'reload keeps the revision',
      'survives icon configuration reloads',
      'head metadata patches without reload',
      'Selected config edits clear diagnostics',
    ])
    expect(titles.map((declared) => declared.line)).toEqual([1, 3, 9, 13, 18])
    expect(unreadable).toEqual([
      { path: 'e2e/example.spec.ts', line: 21, detail: 'names a case without a title this audit can read' },
    ])
  })
})
