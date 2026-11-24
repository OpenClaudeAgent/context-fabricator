import Foundation

enum AppLanguage: String, CaseIterable {
  case system
  case en
  case fr
  case de
  case es
  case it
  case pt
  case ja
  case zhHans = "zh-Hans"

  static let preferenceKey = "app-language"

  var name: String {
    switch self {
    case .system: ""
    case .en: "English"
    case .fr: "Français"
    case .de: "Deutsch"
    case .es: "Español"
    case .it: "Italiano"
    case .pt: "Português"
    case .ja: "日本語"
    case .zhHans: "简体中文"
    }
  }

  func locale(system: Locale = .autoupdatingCurrent) -> Locale {
    guard self != .system else { return system }
    let selected = Locale.Components(identifier: rawValue).languageComponents
    var components = Locale.Components(locale: system)
    components.languageComponents.languageCode = selected.languageCode
    components.languageComponents.script = selected.script
    return Locale(components: components)
  }
}

extension Locale {
  func localized(_ resource: LocalizedStringResource) -> String {
    var resource = resource
    resource.locale = self
    return String(localized: resource)
  }

  func localizedKey(_ key: String, in bundle: Bundle = .main) -> String {
    let preferences = [language.minimalIdentifier]
    let preferred = Bundle.preferredLocalizations(
      from: bundle.localizations, forPreferences: preferences)
    guard let region = preferred.first,
      let url = bundle.url(forResource: region, withExtension: "lproj"),
      let resources = Bundle(url: url)
    else { return key }
    return resources.localizedString(forKey: key, value: key, table: nil)
  }
}
