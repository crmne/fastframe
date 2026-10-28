# frozen_string_literal: true

require 'open3'
require 'pathname'

# Builds the Crates and Reference pages from the repository's own Markdown,
# so each crate's README stays the one place its documentation lives:
#
# - one page per crate, in the order of the workspace README's crate table,
#   described by that table's row;
# - the migration guide from MIGRATION.md;
# - the version label from the workspace version in Cargo.toml.
#
# Runs before the theme's post_read hook, which reads the collections to build
# the sidebar, the search index and llms.txt.
module FastframeDocs
  module Sources
    module_function

    REPOSITORY = 'https://github.com/crmne/fastframe'
    CRATE_ROW = /^\|\s*\[`(fastframe-[a-z0-9-]+)`\]\(crates\/\1\/?\)\s*\|\s*(.+?)\s*\|\s*$/
    LINK = /\]\(([^)\s]+)\)/

    def apply(site)
      root = File.join(site.source, '..')
      crates = crate_table(File.read(File.join(root, 'README.md')))

      crates.each_with_index do |(name, description), index|
        add(site, 'crates', "crates/#{name}/README.md", crates.keys,
            'title' => name,
            'description' => plain(description),
            'permalink' => "/#{name}/",
            'nav_order' => index + 1)
      end

      add(site, 'reference', 'MIGRATION.md', crates.keys,
          'permalink' => '/moving-apps/',
          'nav_order' => 1)

      version = workspace_version(File.read(File.join(root, 'Cargo.toml')))
      site.config['jekyll_vitepress']['version'] = { 'value' => version } if version
    end

    def add(site, collection_name, relative, crate_names, data)
      collection = site.collections.fetch(collection_name)
      # Left unexpanded, so the document's relative path is ../<relative> and
      # the theme's edit link (docs/:path) lands on the file itself.
      path = File.join(site.source, '..', relative)
      title, body = split_title(File.read(path))

      doc = Jekyll::Document.new(path, site: site, collection: collection)
      doc.content = rewrite_links(body, File.dirname(relative), crate_names)
      doc.data.merge!(
        'layout' => 'default',
        'title' => title,
        'description' => first_paragraph(body),
        'render_with_liquid' => false
      )
      committed = last_commit_time(path)
      doc.data['last_updated_at'] = committed if committed
      doc.data.merge!(data)
      collection.docs << doc
    end

    # The theme dates pages by Git only inside the site source (docs/), so
    # these files from the rest of the repository are dated here, for the
    # "Last updated" footer and the sitemap's lastmod. Nil when Git cannot
    # date the file (untracked, or no history).
    def last_commit_time(path)
      output, status = Open3.capture2('git', '-C', File.dirname(path), 'log', '-1', '--format=%ct', '--',
                                      File.basename(path), err: File::NULL)
      Time.at(Integer(output.strip)).utc if status.success? && !output.strip.empty?
    end

    # The workspace README's crate table, in order: name => description.
    def crate_table(readme)
      readme.each_line.filter_map do |line|
        match = CRATE_ROW.match(line)
        [match[1], match[2]] if match
      end.to_h
    end

    def workspace_version(cargo_toml)
      section = cargo_toml[/^\[workspace\.package\]\s*\n(.*?)(?=^\[|\z)/m, 1]
      section && section[/^version\s*=\s*"([^"]+)"/, 1]
    end

    # The page title comes from the leading "# Heading"; the theme draws it.
    def split_title(markdown)
      match = markdown.match(/\A\s*#\s+(.+?)\s*\n/)
      return [nil, markdown] unless match

      [match[1], match.post_match.sub(/\A\s+/, '')]
    end

    def first_paragraph(markdown)
      paragraph = markdown.split(/\n\s*\n/).find { |block| block.match?(/\A[[:alpha:]`*\[]/) }
      paragraph && plain(paragraph.tr("\n", ' '))
    end

    def plain(text)
      text.gsub(/\[([^\]]+)\]\([^)]+\)/, '\1').delete('`*').squeeze(' ').strip
    end

    # Links written for GitHub, relative to the file: other crates and the
    # migration guide become site pages; anything else in the repository
    # opens on GitHub.
    def rewrite_links(markdown, directory, crate_names)
      markdown.gsub(LINK) do
        target = Regexp.last_match(1)
        "](#{site_link(target, directory, crate_names)})"
      end
    end

    def site_link(target, directory, crate_names)
      return target if target.match?(%r{\A([a-z]+:|#|/)})

      file, anchor = target.split('#', 2)
      resolved = Pathname.new(directory).join(file).cleanpath.to_s
      suffix = anchor ? "##{anchor}" : ''

      crate = resolved[%r{\Acrates/([^/]+)(?:/README\.md)?/?\z}, 1]
      return "/#{crate}/#{suffix}" if crate && crate_names.include?(crate)
      return "/moving-apps/#{suffix}" if resolved == 'MIGRATION.md'

      "#{REPOSITORY}/blob/main/#{resolved}#{suffix}"
    end
  end
end

Jekyll::Hooks.register :site, :post_read, priority: :high do |site|
  FastframeDocs::Sources.apply(site)
end
