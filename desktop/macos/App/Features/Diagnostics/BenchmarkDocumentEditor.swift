import SwiftUI

struct BenchmarkDocumentEditor: View {
  @Environment(\.dismiss) private var dismiss
  @State private var path = ""
  @State private var text = ""
  @State private var feedback: String?

  let add: (PipelineBenchmark.Document) throws -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Add Text to Temporary Corpus").font(.title2.bold())
      TextField("Relative path", text: $path, prompt: Text("src/example.rs"))
        .textFieldStyle(.roundedBorder)
        .accessibilityIdentifier("benchmark-document-path")
      TextEditor(text: $text)
        .font(.body.monospaced())
        .accessibilityLabel("Document text")
        .accessibilityIdentifier("benchmark-document-text")
        .frame(minHeight: 190)
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.quaternary))
      if let feedback {
        Label(feedback, systemImage: "exclamationmark.circle").foregroundStyle(.orange)
      }
      HStack {
        Spacer()
        Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
        Button("Add") {
          do {
            try add(.init(path: path.trimmingCharacters(in: .whitespacesAndNewlines), text: text))
            dismiss()
          } catch {
            feedback = error.localizedDescription
          }
        }
        .buttonStyle(.borderedProminent)
        .accessibilityIdentifier("benchmark-add-document")
        .disabled(path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || text.isEmpty)
      }
    }
    .padding(18)
    .frame(minWidth: 480, idealWidth: 640, minHeight: 320)
  }
}
