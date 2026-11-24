import LocalAPI
import SwiftUI

struct AnswerLanguagePicker: View {
  @Environment(\.locale) private var locale
  let options: [Components.Schemas.AnswerLanguageOption]
  let selection: String
  let select: (String) -> Void

  @State private var presented = false
  @State private var query = ""
  @FocusState private var searching: Bool

  private var selectedLanguage: String {
    options.first { $0.code == selection }.map(displayName) ?? locale.localized("Choose language")
  }

  private var matches: [Components.Schemas.AnswerLanguageOption] {
    let search = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !search.isEmpty else { return options }
    return options.filter {
      displayName($0).localizedStandardContains(search)
        || $0.name.localizedStandardContains(search)
        || $0.nativeName.localizedStandardContains(search)
        || $0.code.caseInsensitiveCompare(search) == .orderedSame
    }
  }

  var body: some View {
    Button {
      query = ""
      presented = true
    } label: {
      HStack(spacing: 6) {
        Label(selectedLanguage, systemImage: "globe")
        Image(systemName: "chevron.up.chevron.down")
          .font(.caption2)
          .foregroundStyle(.secondary)
      }
    }
    .accessibilityLabel("Answer language")
    .accessibilityValue(selectedLanguage)
    .accessibilityIdentifier("answer-language-picker")
    .popover(isPresented: $presented, arrowEdge: .trailing) {
      VStack(spacing: 8) {
        searchField
        Divider()
        languageList
      }
      .padding(12)
      .frame(width: 300, height: 340)
      .defaultFocus($searching, true)
    }
  }

  private var searchField: some View {
    HStack(spacing: 8) {
      Image(systemName: "magnifyingglass")
        .foregroundStyle(.secondary)
        .accessibilityHidden(true)
      TextField("Search languages", text: $query)
        .textFieldStyle(.plain)
        .multilineTextAlignment(.leading)
        .focused($searching)
        .accessibilityIdentifier("answer-language-search")
      if !query.isEmpty {
        Button("Clear Search", systemImage: "xmark.circle.fill") { query = "" }
          .labelStyle(.iconOnly)
          .buttonStyle(.plain)
          .foregroundStyle(.secondary)
      }
    }
    .padding(.horizontal, 8)
    .padding(.vertical, 4)
  }

  private var languageList: some View {
    ScrollView {
      LazyVStack(spacing: 2) {
        if matches.isEmpty {
          Text("No matching languages")
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, minHeight: 72)
        }
        ForEach(matches, id: \.code) { language in
          languageRow(language)
        }
      }
    }
  }

  private func displayName(_ language: Components.Schemas.AnswerLanguageOption) -> String {
    language.code == "auto" ? locale.localized("Automatic") : language.nativeName
  }

  private func languageRow(_ language: Components.Schemas.AnswerLanguageOption) -> some View {
    Button {
      select(language.code)
      presented = false
    } label: {
      HStack(spacing: 10) {
        Image(systemName: "checkmark")
          .frame(width: 14)
          .foregroundStyle(Color.accentColor)
          .opacity(language.code == selection ? 1 : 0)
          .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 2) {
          Text(displayName(language))
          if language.code == "auto" {
            Text("Match the question").font(.caption).foregroundStyle(.secondary)
          } else if language.name != language.nativeName {
            Text(language.name).font(.caption).foregroundStyle(.secondary)
          }
        }
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 8)
      .padding(.vertical, 5)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(language.code == selection ? [.isSelected] : [])
    .accessibilityIdentifier("answer-language-\(language.code)")
  }
}
