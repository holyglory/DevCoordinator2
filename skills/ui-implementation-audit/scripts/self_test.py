#!/usr/bin/env python3
"""Fixture-based smoke tests for the UI implementation audit harness."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "scripts" / "build_ui_implementation_audit_batches.py"
VERIFY = ROOT / "scripts" / "verify_ui_implementation_audit_results.py"
IMPORT_FORMAL = ROOT / "scripts" / "import_formal_web_evidence.py"
TIMEOUT_SECONDS = int(os.environ.get("UI_IMPLEMENTATION_AUDIT_SELF_TEST_TIMEOUT", "30"))
KEEP_TEMP_ON_FAILURE = os.environ.get("UI_IMPLEMENTATION_AUDIT_SELF_TEST_KEEP_TEMP", "").lower() in {"1", "true", "yes", "on"}
PNG_1X1 = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000a49444154789c6360000002000100ffff03000006000557bfab9d00000000"
    "49454e44ae426082"
)


INTERACTION_CHECKLIST_LINE = (
    "Interaction checklist: badge-detail=pass; row-hit-target=pass; "
    "navigation-cursor=pass; transient-disclosure=pass; disclosure-scrollbar=pass; "
    "icon-meaning=pass; stable-expansion-width=pass; hover-copy=pass; "
    "status-summary=pass; message-metadata=pass."
)


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def write_bytes(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)


def run(args: list[str], *, expect: int = 0) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            args,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout or ""
        stderr = exc.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", errors="replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        raise AssertionError(
            f"Command timed out after {TIMEOUT_SECONDS}s: {' '.join(args)}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ) from exc
    if result.returncode != expect:
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        raise AssertionError(f"Expected exit {expect}, got {result.returncode}: {' '.join(args)}")
    return result


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def make_ui_fixture(root: Path) -> None:
    write(
        root / "package.json",
        """{
  "scripts": {
    "dev": "vite --host 127.0.0.1",
    "test:visual": "playwright test"
  },
  "dependencies": {
    "@vitejs/plugin-react": "latest",
    "vite": "latest",
    "react": "latest",
    "react-dom": "latest"
  },
  "devDependencies": {
    "@playwright/test": "latest"
  }
}
""",
    )
    write(
        root / "src" / "App.tsx",
        """export function Dashboard() {
  return (
    <main className="dashboard-shell">
      <nav aria-label="Primary navigation">
        <a href="/reports">Reports</a>
        <a href="/settings">Settings</a>
      </nav>
      <section className="hero">
        <h1>Operations Dashboard</h1>
        <p>Review urgent incidents before routine archive details.</p>
        <button type="button">Resolve incident</button>
      </section>
      <aside className="rare-detail">Audit archive exported monthly.</aside>
    </main>
  );
}
""",
    )
    write(
        root / "src" / "styles.css",
        """.dashboard-shell {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 20rem;
  gap: 16px;
}

.hero {
  padding: 24px;
}
""",
    )
    write(
        root / "docs" / "journeys.md",
        """# Dashboard User Journey

Target user: operations lead.
Goal: review urgent incidents and resolve the highest priority item first.
Screen sequence: dashboard -> incident details -> resolution confirmation.
Primary action: Resolve incident.
Responsive requirement: mobile must show the active incident summary and action before archive detail.
Acceptance criteria: desktop and mobile screenshots match the dashboard mockup hierarchy.
""",
    )
    write_bytes(root / "design" / "mockups" / "dashboard-mobile.png", PNG_1X1)
    write(root / "design" / "mockups" / "dashboard.html", "<main><h1>Dashboard concept</h1></main>\n")
    write_bytes(root / "public" / "logo.png", PNG_1X1)


def make_currency_rates_fixture(root: Path) -> None:
    write(root / "package.json", '{"scripts":{"dev":"vite --host 127.0.0.1","test:visual":"playwright test"},"dependencies":{"vite":"latest","react":"latest","react-dom":"latest"},"devDependencies":{"@playwright/test":"latest"}}\n')
    write(
        root / "src" / "CurrencyRatesPage.tsx",
        """export function CurrencyRatesPage() {
  return (
    <main className="rates-page">
      <section className="target-settings" aria-label="Target currency settings">
        <h1>Currency Rates</h1>
        <label>
          Target Currency
          <select defaultValue="USD"><option>USD</option><option>EUR</option></select>
        </label>
        <button type="button">Apply settings</button>
      </section>
      <section className="most-used-rates" aria-label="Most-used live currency rates">
        <h2>Most-used rates</h2>
        <article>EUR/USD 1.08</article>
        <article>GBP/USD 1.27</article>
      </section>
    </main>
  );
}
""",
    )
    write(
        root / "src" / "rates.css",
        """.rates-page {
  display: grid;
  gap: 16px;
}

@media (max-width: 600px) {
  .target-settings {
    min-height: 82vh;
    order: 1;
  }

  .most-used-rates {
    order: 2;
  }
}
""",
    )
    write(
        root / "docs" / "currency-rates-journey.md",
        """# Currency Rates Journey

Primary user goal: quickly decide current exchange rates for the currencies used most often.
Primary decision: decide current exchange rates for the currencies used most often.
Required facts: the most-used live rates list.
Frequent action: inspect a rate and continue cost tracking.
Occasional control: adjust target currency.
Rare control: advanced target/settings configuration.
UI audit handoff: verify the rendered surface supports the rate decision and does not let target/settings controls overwhelm the decision path.
""",
    )
    write_bytes(root / "design" / "mockups" / "currency-rates-mobile.png", PNG_1X1)


def make_cli_fixture(root: Path) -> None:
    write(root / "README.md", "# CLI fixture\n\nNo rendered UI surface.\n")
    write(root / "src" / "tool.py", "def main():\n    return 1\n")
    write(
        root / "views" / "StatusScreen.py",
        "state = {'data': [], 'items': []}\nHTML = \"<main><h1>False UI</h1><button>Run</button></main>\"\n",
    )
    write(
        root / "src" / "ledger.py",
        "def find_ledger_violations(text):\n    return []\n\ndef audit_ledger(text):\n    return find_ledger_violations(text)\n",
    )
    write(
        root / "src" / "ledger_view_model.py",
        "class LedgerViewModel:\n    pass\n\ndef audit_ledger():\n    return LedgerViewModel()\n",
    )


def make_evidence_only_fixture(root: Path) -> None:
    write(
        root / "docs" / "mockups" / "contacts.html",
        "<main><h1>Contacts</h1><button>Add contact</button></main>\n",
    )
    write(root / "src" / "theme.css", "main { display: grid; }\n")
    write_bytes(root / "docs" / "mockups" / "contacts.png", PNG_1X1)


def make_story_only_fixture(root: Path) -> None:
    write(
        root / "stories" / "ContactList.stories.tsx",
        "export const Example = () => <main><h1>Contacts</h1></main>;\n",
    )


def make_starter_fixture(root: Path) -> None:
    write(
        root / "src" / "App.tsx",
        """export function App() {
  return <main><h1>Vite + React</h1><button>count is 0</button><p>Edit src/App.tsx and save to test HMR</p></main>;
}
""",
    )


def make_native_ui_fixture(root: Path) -> None:
    write(
        root / "Sources" / "ContactsView.swift",
        """import SwiftUI

struct ContactsView: View {
    var body: some View {
        List {
            Text("Ada Lovelace")
            Button("Add contact") {}
        }
    }
}
""",
    )


def make_gate_precision_fixture(root: Path) -> None:
    write(root / "src" / "SignOut.tsx", "export const SignOut = () => <button onClick={signOut}>Sign out</button>;\n")
    write(
        root / "src" / "Dashboard.tsx",
        "export const Dashboard = () => <DashboardShell><ContactToolbar /><ContactList /></DashboardShell>;\n",
    )
    write(
        root / "src" / "VueDashboard.vue",
        "<template><dashboard-shell><contact-list /></dashboard-shell></template>\n",
    )
    write(
        root / "src" / "design" / "components" / "ContactList.tsx",
        "export const ContactList = () => <section><h1>Contacts</h1></section>;\n",
    )
    write(
        root / "src" / "BrandedPage.tsx",
        "export const BrandedPage = () => <main><h1>Vite + React</h1><button onClick={deploy}>Deploy release</button></main>;\n",
    )
    write(
        root / "src" / "CustomizedStarter.tsx",
        "export const App = ({ contacts }) => <main><h1>Vite + React</h1><p>Edit src/App.tsx and save to test HMR</p><ul>{contacts.map(contact => <li>{contact.name}</li>)}</ul></main>;\n",
    )
    write(
        root / "src" / "DesignSystemContacts.tsx",
        "export const Contacts = ({ contacts }) => <Stack><Heading>Contacts</Heading><ContactList contacts={contacts} /></Stack>;\n",
    )
    write(
        root / "src" / "NativeContacts.tsx",
        "export const Contacts = ({ contacts }) => <View><Text>Contacts</Text><FlatList data={contacts} renderItem={renderContact} /></View>;\n",
    )
    write(
        root / "src" / "WelcomeEmail.ts",
        "export const welcomeEmail = { template: `<main><h1>Welcome</h1><p>Your account is ready.</p></main>` };\n",
    )
    write(
        root / "src" / "TaggedWelcomeEmail.ts",
        "import { html } from 'email-template';\nexport const welcomeEmail = html`<main><h1>Verify your account</h1><button>Verify email</button></main>`;\n",
    )
    write(
        root / "src" / "LitTaggedWelcomeEmail.ts",
        "import { html } from 'lit-html';\nexport const welcomeEmail = html`<main><h1>Verify your account</h1><button>Verify email</button></main>`;\n",
    )
    write(
        root / "src" / "LitFunctionalContacts.ts",
        "import { html } from 'lit-html';\nexport const Contacts = (contacts) => html`<main><h1>Contacts</h1><ul>${contacts.map(contact => html`<li>${contact.name}</li>`)}</ul></main>`;\n",
    )
    write(
        root / "src" / "PascalLitWelcomeEmail.ts",
        "import { html } from 'lit-html';\nexport const WelcomeEmail = (user) => html`<main><h1>Welcome</h1><p>${user.name}</p></main>`;\n",
    )
    write(
        root / "src" / "PascalJsxWelcomeEmail.tsx",
        "export const WelcomeEmail = ({ user }) => <main><h1>Welcome</h1><p>{user.name}</p></main>;\n",
    )
    write(
        root / "src" / "email.ts",
        "import { html } from 'lit-html';\nexport function PasswordResetEmailView() { return html`<main><h1>Reset password</h1><p>Follow the link.</p></main>`; }\n",
    )
    write(
        root / "src" / "ComposeEmail.tsx",
        "export const ComposeEmail = () => <form><h1>Compose email</h1><label>Recipient</label><input name=\"recipient\" /><button onClick={send}>Send</button></form>;\n",
    )
    write(
        root / "src" / "Preview.tsx",
        "/* This deliberately long explanatory comment must not make a placeholder count as implementation. */\n"
        "export const Preview = () => <button>Example</button>;\n",
    )
    write(
        root / "Sources" / "ContactStack.swift",
        """import SwiftUI
import UIKit

struct ContactStack: UIViewRepresentable {
    func makeUIView(context: Context) -> UIStackView {
        let stack = UIStackView()
        let button = UIButton(type: .system)
        button.setTitle("Add contact", for: .normal)
        stack.addArrangedSubview(button)
        return stack
    }
    func updateUIView(_ view: UIStackView, context: Context) {}
}
""",
    )
    write(
        root / "src" / "ContactsPanel.java",
        """import javax.swing.JButton;
import javax.swing.JPanel;

public final class ContactsPanel extends JPanel {
    public ContactsPanel() {
        add(new JButton("Add contact"));
    }
}
""",
    )
    write(
        root / "src" / "contact_surface.canvas",
        "use canvas_kit::ContactSurface;\n\npub fn build_contacts() -> ContactSurface {\n    ContactSurface::new()\n}\n",
    )
    write(
        root / "src" / "RouteOnly.tsx",
        "export const RouteOnly = () => <Outlet />;\n",
    )
    write(
        root / "src" / "NullRoute.tsx",
        "export function NullRoute() { return null; }\n",
    )
    write(
        root / "src" / "RootMount.tsx",
        "createRoot(document.querySelector('#root')).render(<App />);\n",
    )
    write(
        root / "src" / "ComingSoon.tsx",
        "export const ComingSoon = () => <main data-testid=\"page\"><h1>Coming Soon</h1><p>Example</p></main>;\n",
    )
    write(
        root / "src" / "TitledPlaceholder.tsx",
        "export const Contacts = () => <main><h1>Contacts</h1><p>Coming soon</p></main>;\n",
    )
    write(
        root / "src" / "StrictRoot.tsx",
        "createRoot(document.querySelector('#root')).render(<React.StrictMode><App /></React.StrictMode>);\n",
    )
    write(
        root / "src" / "AuthRoute.tsx",
        "export const AuthRoute = () => <AuthProvider><Outlet /></AuthProvider>;\n",
    )
    write(
        root / "src" / "WrappedRoot.tsx",
        "createRoot(document.querySelector('#root')).render(<ErrorBoundary><BrowserRouter><App /></BrowserRouter></ErrorBoundary>);\n",
    )
    write(
        root / "src" / "WrappedRoute.tsx",
        "export const Route = () => <RequireAuth><PageLayout><Outlet /></PageLayout></RequireAuth>;\n",
    )
    write(
        root / "src" / "VerbosePlaceholder.tsx",
        "export const Contacts = () => <main><h1>Contacts</h1><p>Coming soon</p><p>We are preparing this page.</p><p>Check back later.</p><footer>Acme</footer></main>;\n",
    )
    write(
        root / "src" / "AccessiblePlaceholder.tsx",
        "export const Contacts = () => <button aria-label=\"Coming soon\" />;\n",
    )
    write(
        root / "src" / "UnderConstruction.tsx",
        "export const Contacts = () => <main><h1>Contacts</h1><p>Under construction</p></main>;\n",
    )
    write(
        root / "src" / "TodoPage.tsx",
        "export const TodoPage = () => <main><h1>Todo</h1><button onClick={addTask}>Add task</button></main>;\n",
    )
    write(
        root / "src" / "DynamicContacts.tsx",
        "export const Contacts = ({ contacts }) => <ul>{contacts.map(c => <li>{c.name}</li>)}</ul>;\n",
    )
    write(root / "src" / "RootScreen.tsx", "export const App = () => <Root><Screen /></Root>;\n")
    write(root / "src" / "LayoutPage.tsx", "export const App = () => <Layout><Page /></Layout>;\n")
    write(
        root / "src" / "CustomAuthRoute.tsx",
        "export const App = () => <RequireAuth><PageLayout><Route /></PageLayout></RequireAuth>;\n",
    )
    write(
        root / "src" / "NoOpPlaceholder.tsx",
        "export const Contacts = () => <main><h1>Contacts</h1><p>Coming soon</p><button onClick={() => {}}>Continue</button></main>;\n",
    )
    write(
        root / "src" / "WorkInProgress.tsx",
        "export const Contacts = () => <main><h1>Contacts</h1><p>Work in progress</p></main>;\n",
    )
    write(
        root / "src" / "ContactsComponent.ts",
        "@Component({template: `<main><h1>Contacts</h1><ul><li *ngFor=\"let contact of contacts\">{{ contact.name }}</li></ul></main>`})\nexport class ContactsComponent {}\n",
    )
    write(
        root / "src" / "ContactsElement.ts",
        "export class ContactsElement extends LitElement { render() { return html`<main><h1>Contacts</h1><ul>${this.contacts.map(c => html`<li>${c.name}</li>`)}</ul></main>`; } }\n",
    )
    write(
        root / "templates" / "contacts.php",
        "<main><h1>Contacts</h1><ul><?php foreach ($contacts as $contact): ?><li><?= $contact->name ?></li><?php endforeach; ?></ul></main>\n",
    )
    write(root / "src" / "PyQtImports.py", "from PyQt6.QtWidgets import QWidget\n")
    write(
        root / "src" / "ContactsWidget.py",
        "from PyQt6.QtWidgets import QPushButton, QVBoxLayout, QWidget\n\nclass ContactsWidget(QWidget):\n    def __init__(self):\n        super().__init__()\n        layout = QVBoxLayout(self)\n        layout.addWidget(QPushButton('Add contact'))\n",
    )
    write(root / "Sources" / "EmptyContactsView.swift", "import SwiftUI\nstruct ContactsView: View {}\n")
    write(
        root / "Sources" / "PlaceholderContactsView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { VStack { Text(\"Contacts\"); Text(\"Coming soon\") } } }\n",
    )
    write(
        root / "Sources" / "EmptyStackView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { VStack {} } }\n",
    )
    write(
        root / "Sources" / "EmptyRepresentable.swift",
        "import UIKit\nstruct EmptyView: UIViewRepresentable { func makeUIView(context: Context) -> UIStackView { UIStackView() }; func updateUIView(_ view: UIStackView, context: Context) {} }\n",
    )
    write(
        root / "src" / "EmptyContactsPanel.java",
        "import javax.swing.JPanel;\npublic final class ContactsPanel extends JPanel {}\n",
    )
    write(
        root / "src" / "LayoutOnlyPanel.java",
        "import javax.swing.JPanel;\npublic final class ContactsPanel extends JPanel { ContactsPanel() { setLayout(null); } }\n",
    )
    write(
        root / "src" / "ContainerOnlyPanel.java",
        "import javax.swing.JPanel;\npublic final class ContactsPanel extends JPanel { ContactsPanel() { add(new JPanel()); } }\n",
    )
    write(
        root / "src" / "LayoutOnlyWidget.py",
        "from PyQt6.QtWidgets import QVBoxLayout, QWidget\nclass ContactsWidget(QWidget):\n    def __init__(self):\n        super().__init__()\n        self.setLayout(QVBoxLayout())\n",
    )
    write(
        root / "Sources" / "EmptyController.swift",
        "import UIKit\nclass ContactsController: UIViewController { override func viewDidLoad() { super.viewDidLoad(); view.addSubview(UIView()) } }\n",
    )
    write(
        root / "Sources" / "PartialContactsView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { VStack { Text(\"Contacts\"); Button(\"Add contact\") {}; Text(\"Export not implemented\") } } }\n",
    )
    write(
        root / "Sources" / "DataBoundPartialContactsView.swift",
        "import SwiftUI\nstruct ContactsView: View { let contacts: [Contact]; var body: some View { List { ForEach(contacts) { contact in Text(contact.name) }; Text(\"Export not implemented\") } } }\n",
    )
    write(
        root / "Sources" / "CustomOnlyContactsView.swift",
        "import SwiftUI\nstruct ContactsScreen: View { var body: some View { ContactsList() } }\n",
    )
    write(
        root / "Sources" / "EmptyTextView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { Text(\"\") } }\n",
    )
    write(
        root / "Sources" / "EmptyVerbatimTextView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { Text(verbatim: \"\") } }\n",
    )
    write(
        root / "Sources" / "WhitespaceVerbatimTextView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { Text(verbatim: \"   \" ) } }\n",
    )
    write(
        root / "Sources" / "EscapedWhitespaceVerbatimTextView.swift",
        "import SwiftUI\nstruct ContactsView: View { var body: some View { Text(verbatim: \"\\t\" ) } }\n",
    )
    write(
        root / "Sources" / "ContactsGridView.swift",
        "import SwiftUI\nstruct ContactsView: View { let contacts: [Contact]; var body: some View { ContactsGrid(items: contacts) } }\n",
    )
    write(
        root / "Sources" / "ContactRowContainer.swift",
        "import SwiftUI\nstruct ContactsView: View { let contact: Contact; var body: some View { VStack { ContactRow(contact: contact) } } }\n",
    )
    write(
        root / "Sources" / "ReadOnlyContactsController.swift",
        "import UIKit\nclass ContactsController: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let label = UILabel(); label.text = \"Contacts\"; view.addSubview(label) } }\n",
    )
    write(
        root / "Sources" / "ContactsNativeView.swift",
        "import UIKit\nfinal class ContactsNativeView: UIView { override init(frame: CGRect) { super.init(frame: frame); let label = UILabel(); label.text = \"Contacts\"; addSubview(label) }; required init?(coder: NSCoder) { fatalError() } }\n",
    )
    write(
        root / "Sources" / "UnusedButtonController.swift",
        "import UIKit\nclass ContactsController: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let button = UIButton(type: .system) } }\n",
    )
    write(
        root / "Sources" / "BlankAttachedButtonController.swift",
        "import UIKit\nclass EmptyVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let blank = UIButton(); view.addSubview(blank) } }\n",
    )
    write(
        root / "Sources" / "BlankButtonWithDebugTextController.swift",
        "import UIKit\nclass EmptyVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let text = \"debug\"; let blank = UIButton(); view.addSubview(blank) } }\n",
    )
    write(
        root / "Sources" / "TypedContactsController.swift",
        "import UIKit\nclass ContactsController: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let label: UILabel = UILabel(); label.text = \"Contacts\"; view.addSubview(label) } }\n",
    )
    write(
        root / "Sources" / "TypedContactsAppKitController.swift",
        "import AppKit\nclass ContactsController: NSViewController { override func loadView() { view = NSView(); let label: NSTextField = NSTextField(labelWithString: \"Contacts\"); view.addSubview(label) } }\n",
    )
    write(
        root / "Sources" / "ProfileImageController.swift",
        "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let photo = UIImageView(image: avatar); view.addSubview(photo) } }\n",
    )
    write(
        root / "Sources" / "EmptyProfileImageController.swift",
        "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let photo = UIImageView(image: UIImage()); view.addSubview(photo) } }\n",
    )
    write(
        root / "Sources" / "NamedProfileImageController.swift",
        "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let photo = UIImageView(image: UIImage(named: \"avatar\")); view.addSubview(photo) } }\n",
    )
    write(
        root / "Sources" / "EmptyDataProfileImageController.swift",
        "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let photo = UIImageView(image: UIImage(data: Data())); view.addSubview(photo) } }\n",
    )
    write(
        root / "Sources" / "DataProfileImageController.swift",
        "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let photo = UIImageView(image: UIImage(data: avatarBytes)); view.addSubview(photo) } }\n",
    )
    write(
        root / "Sources" / "AssignedEmptyProfileImageController.swift",
        "import UIKit\nclass ProfileVC: UIViewController { override func viewDidLoad() { super.viewDidLoad(); let photo = UIImageView(); photo.image = UIImage(); view.addSubview(photo) } }\n",
    )
    write(
        root / "src" / "unrelated_surface.canvas",
        "use canvas_kit::ContactSurface;\n\npub fn audit_ledger() -> usize { 0 }\npub fn build_contacts() -> ContactSurface { ContactSurface::new() }\n",
    )
    write(
        root / "templates" / "contacts.html",
        "<main><h1>Contacts</h1><ul><li>Ada Lovelace</li></ul></main>\n",
    )
    write(
        root / "src" / "contacts_flask.py",
        "from flask import Flask\napp = Flask(__name__)\n@app.route('/contacts')\ndef contacts():\n    return '''<main><h1>Contacts</h1><ul><li>Ada Lovelace</li></ul></main>'''\n",
    )
    write(
        root / "src" / "contacts_flask_get.py",
        "from flask import Flask\napp = Flask(__name__)\n@app.get('/contacts')\ndef contacts():\n    return '<main><h1>Contacts</h1><p>Ada Lovelace</p></main>'\n",
    )
    write(
        root / "src" / "contacts_blueprint.py",
        "from flask import Blueprint\nbp = Blueprint('contacts', __name__)\n@bp.route('/contacts')\ndef contacts():\n    return '<main><h1>Contacts</h1><p>Ada Lovelace</p></main>'\n",
    )
    write(
        root / "src" / "contacts_fastapi.py",
        "from fastapi import FastAPI\nfrom fastapi.responses import HTMLResponse\napp = FastAPI()\n@app.get('/contacts', response_class=HTMLResponse)\ndef contacts():\n    return HTMLResponse('<main><h1>Contacts</h1><p>Ada Lovelace</p></main>')\n",
    )
    write(
        root / "src" / "contacts_fastapi_keyword.py",
        "from fastapi import FastAPI\nfrom fastapi.responses import HTMLResponse\napp = FastAPI()\n@app.get('/contacts', response_class=HTMLResponse)\ndef contacts():\n    return HTMLResponse(content='<main><h1>Contacts</h1><p>Ada Lovelace</p></main>')\n",
    )
    write(
        root / "src" / "contacts_fastapi_status_keyword.py",
        "from fastapi import FastAPI\nfrom fastapi.responses import HTMLResponse\napp = FastAPI()\n@app.get('/contacts', response_class=HTMLResponse)\ndef contacts():\n    return HTMLResponse(status_code=200, content='<main><h1>Contacts</h1><p>Ada Lovelace</p></main>')\n",
    )
    write(
        root / "src" / "contacts_fastapi_fstring.py",
        "from fastapi import FastAPI\nfrom fastapi.responses import HTMLResponse\napp = FastAPI()\n@app.get('/contacts', response_class=HTMLResponse)\ndef contacts(name):\n    return HTMLResponse(content=f'<main><h1>Contacts</h1><p>{name}</p></main>')\n",
    )
    write(
        root / "src" / "contacts_fastapi_headers.py",
        "from fastapi import FastAPI\nfrom fastapi.responses import HTMLResponse\napp = FastAPI()\n@app.get('/contacts', response_class=HTMLResponse)\ndef contacts(name):\n    return HTMLResponse(headers=make_headers(), content=f'<main><h1>Contacts</h1><p>{name}</p></main>')\n",
    )
    write(
        root / "templates" / "contact-paragraphs.php",
        "<?php foreach ($contacts as $contact): ?><p><?= $contact->name ?></p><?php endforeach; ?>\n",
    )
    write(
        root / "templates" / "contact-spans.php",
        "<?php foreach ($contacts as $contact): ?><span><?= $contact->name ?></span><?php endforeach; ?>\n",
    )
    write(
        root / "templates" / "contact-divs.php",
        "<?php foreach ($contacts as $contact): ?><div><?= $contact->name ?></div><?php endforeach; ?>\n",
    )
    write(root / "templates" / "empty-button.html", "<button></button>\n")
    write(root / "src" / "StreamlitConfig.py", "import streamlit as st\nst.set_page_config(page_title='Contacts')\n")
    write(root / "src" / "EmptyGradio.py", "import gradio as gr\nwith gr.Blocks():\n    pass\n")
    write(root / "src" / "StreamlitPlaceholder.py", "import streamlit as st\nst.title('Coming soon')\n")
    write(root / "src" / "GradioPlaceholder.py", "import gradio as gr\ngr.Markdown('Coming soon')\n")
    write(
        root / "src" / "DomPlaceholder.js",
        "const root = document.createElement('main'); root.textContent = 'Coming soon'; document.body.appendChild(root);\n",
    )
    write(
        root / "res" / "layout" / "placeholder.xml",
        "<LinearLayout><TextView android:text=\"Coming soon\" /></LinearLayout>\n",
    )
    write(root / "templates" / "empty-attributed-button.html", "<button type=\"button\"></button>\n")
    write(root / "templates" / "empty-link.html", "<a href=\"/\"></a>\n")
    write(
        root / "src" / "EmptyTk.py",
        "from tkinter import Frame, Tk\nroot = Tk()\nFrame(root).pack()\n",
    )
    write(
        root / "src" / "contacts_imgui.cpp",
        "void ContactsScreen() { ImGui::Begin(\"Contacts\"); ImGui::Button(\"Add contact\"); ImGui::End(); }\n",
    )
    write(
        root / "src" / "partial_contacts.canvas",
        "use canvas_kit::ContactSurface;\n\npub fn render_contacts(contacts: &[Contact]) -> ContactSurface {\n    contacts.iter().map(|contact| contact.name.clone()).collect();\n    let surface = ContactSurface::new();\n    surface.notice(\"Export not implemented\");\n    surface\n}\n",
    )
    write(
        root / "src" / "dynamic_partial_contacts.canvas",
        "use canvas_kit::ContactSurface;\npub fn build_contacts(contacts: Contacts) -> ContactSurface { ContactSurface::new().title(\"Contacts\").rows(contacts).message(\"Export not implemented\") }\n",
    )
    write(
        root / "src" / "local_report_view.py",
        "class ReportView:\n    pass\n\ndef render_report():\n    return ReportView()\n",
    )
    write(
        root / "src" / "database_view.canvas",
        "use database::MaterializedView;\npub fn build_contacts(records: Records) -> MaterializedView { MaterializedView::new().load(records) }\n",
    )
    write(
        root / "src" / "sqlx_view.canvas",
        "use sqlx::ContactView;\npub fn build_contacts(records: Records) -> ContactView { ContactView::new().load(records) }\n",
    )
    write(
        root / "src" / "sea_orm_view.canvas",
        "use sea_orm::ContactView;\npub fn build_contacts(records: Records) -> ContactView { ContactView::new().load(records) }\n",
    )
    write(
        root / "src" / "imported_database_view.py",
        "from database import ReportView\ndef build_contacts(records):\n    return ReportView(records)\n",
    )
    write(
        root / "src" / "UnusedSwingButton.java",
        "import javax.swing.JButton;\nimport javax.swing.JPanel;\npublic final class ContactsPanel extends JPanel { ContactsPanel() { JButton button = new JButton(\"\"); } }\n",
    )
    write(
        root / "src" / "BlankAttachedSwingButton.java",
        "import javax.swing.*; public class Empty extends JPanel { public Empty(){ JButton blank = new JButton(\"\"); add(blank); } }\n",
    )
    write(
        root / "src" / "BlankSwingButtonWithDebugText.java",
        "import javax.swing.*; public class Empty extends JPanel { public Empty(){ String text = \"debug\"; JButton blank = new JButton(\"\"); add(blank); } }\n",
    )
    write(
        root / "src" / "UnusedQtLabel.py",
        "from PyQt6.QtWidgets import QLabel, QWidget\nclass ContactsWidget(QWidget):\n    def __init__(self):\n        super().__init__()\n        label = QLabel('')\n",
    )
    write(
        root / "src" / "BlankAttachedQtLabel.py",
        "from PyQt6.QtWidgets import QWidget, QLabel, QVBoxLayout\nclass EmptyWidget(QWidget):\n    def __init__(self):\n        super().__init__(); layout=QVBoxLayout(self); blank=QLabel(''); layout.addWidget(blank)\n",
    )
    write(
        root / "src" / "BlankQtLabelWithDebugText.py",
        "from PyQt6.QtWidgets import QWidget, QLabel, QVBoxLayout\nclass EmptyWidget(QWidget):\n    def __init__(self):\n        super().__init__(); text='debug'; layout=QVBoxLayout(self); blank=QLabel(''); layout.addWidget(blank)\n",
    )
    write(root / "src" / "EmptyCompose.kt", "@Composable fun EmptyScreen() { Text(\"\") }\n")
    write(
        root / "src" / "EmptyAliasCompose.kt",
        "@Composable fun EmptyScreen() { val title = \"\"; Text(title) }\n",
    )
    write(
        root / "src" / "EmptyConstructorAliasCompose.kt",
        "@Composable fun EmptyScreen() { val title = String(); Text(title) }\n",
    )
    write(
        root / "src" / "EmptyAliasChainCompose.kt",
        "@Composable fun EmptyScreen() { val blank = \"\"; val title = blank; Text(title) }\n",
    )
    write(
        root / "src" / "EmptyComposeWithDebugText.kt",
        "@Composable fun EmptyScreen() { val text = \"debug\"; Text(\"\") }\n",
    )
    write(root / "src" / "DynamicCompose.kt", "@Composable fun Contacts(name: String) { Text(name) }\n")
    write(
        root / "src" / "EmptyFlutter.dart",
        "class Empty extends StatelessWidget { Widget build(BuildContext context) { return Text(\"\"); } }\n",
    )
    write(
        root / "src" / "EmptyAliasFlutter.dart",
        "class Empty extends StatelessWidget { Widget build(BuildContext context) { const title = \"\"; return Text(title); } }\n",
    )
    write(
        root / "src" / "DynamicFlutter.dart",
        "class Contacts extends StatelessWidget { Widget build(BuildContext context) { return Text(title); } }\n",
    )
    write(root / "src" / "EmptyStreamlit.py", "import streamlit as st\nst.title('')\n")
    write(
        root / "src" / "EmptyAliasStreamlit.py",
        "import streamlit as st\ntitle = ''\nst.title(title)\n",
    )
    write(
        root / "src" / "EmptyStreamlitWithDebugText.py",
        "import streamlit as st\ntext = 'debug'\nst.title('')\n",
    )
    write(root / "src" / "DynamicStreamlit.py", "import streamlit as st\nst.dataframe(rows)\n")
    write(root / "src" / "EmptyGradioMarkdown.py", "import gradio as gr\ngr.Markdown('')\n")
    write(
        root / "src" / "EmptyTkButton.py",
        "from tkinter import *\nroot = Tk()\nButton(root, text='').pack()\n",
    )
    write(
        root / "src" / "EmptyDomNode.js",
        "const node = document.createElement('div'); node.textContent = ''; document.body.appendChild(node);\n",
    )
    write(
        root / "src" / "EmptyAliasDomNode.js",
        "const value = ''; const node = document.createElement('div'); node.textContent = value; document.body.appendChild(node);\n",
    )
    write(
        root / "src" / "EmptyDomNodeWithDebugText.js",
        "const debug = { text: 'debug' }; const node = document.createElement('div'); node.textContent = ''; document.body.appendChild(node);\n",
    )
    write(
        root / "src" / "DynamicDomNode.js",
        "const node = document.createElement('div'); node.textContent = contact.name; document.body.appendChild(node);\n",
    )
    write(root / "res" / "layout" / "empty-control.xml", "<LinearLayout><Button /></LinearLayout>\n")
    write(root / "res" / "layout" / "empty-list.xml", "<RecyclerView />\n")
    write(root / "res" / "layout" / "bound-list.xml", "<ListView ItemsSource=\"{Binding Contacts}\" />\n")
    write(root / "res" / "layout" / "whitespace-list.xml", "<ListView ItemsSource=\"   \" />\n")


def build(
    repo: Path,
    out: Path,
    *extra: str,
    evidence: str | None = None,
    override: tuple[str, str, str] | None = None,
    platform: str | None = None,
) -> dict:
    candidates = ("src/App.tsx", "src/CurrencyRatesPage.tsx", "Sources/ContactsView.swift")
    check(not (evidence and override), "build helper accepts one implementation evidence mode")
    evidence = evidence or (None if override else next((item for item in candidates if (repo / item).is_file()), None))
    check(evidence is not None or override is not None, f"build helper needs implemented UI evidence for {repo}")
    evidence_args = (
        ["--implemented-ui-override", *override]
        if override is not None
        else ["--implemented-ui-file", evidence]
    )
    evidence_path = override[0] if override is not None else str(evidence)
    platform = platform or ("native" if Path(evidence_path).suffix.lower() in {".swift", ".java", ".dart", ".xaml", ".axaml"} else "web")
    platform_args = ["--ui-platform", platform]
    if platform in {"web", "hybrid"}:
        formal_config = repo / "formal-web-ui.json"
        if not formal_config.exists():
            write(formal_config, json.dumps({"targetDefaults": {"journeys": [], "regions": []}, "targets": []}, indent=2))
        platform_args.extend(["--formal-config", "formal-web-ui.json"])
    run(
        [
            sys.executable,
            str(BUILD),
            "--repo",
            str(repo),
            "--out",
            str(out),
            "--run-id",
            "selftest-run",
            *evidence_args,
            *platform_args,
            *extra,
        ]
    )
    return json.loads((out / "manifest.json").read_text(encoding="utf-8"))


def assert_inapplicable(repo: Path, out: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    result = run(
        [sys.executable, str(BUILD), "--repo", str(repo), "--out", str(out), "--run-id", "selftest-run", *extra],
        expect=3,
    )
    check(not out.exists(), f"inapplicable audit must not create artifacts: {out}")
    return result


def verify(out: Path, *, expect: int = 0) -> subprocess.CompletedProcess[str]:
    return run([sys.executable, str(VERIFY), "--manifest", str(out / "manifest.json"), "--reports", str(out / "reports")], expect=expect)


def complete_ledger(out: Path, manifest: dict) -> None:
    path = out / "execution_ledger.json"
    ledger = json.loads(path.read_text(encoding="utf-8"))
    ledger["worker_capability_check"].update(
        {
            "status": "completed",
            "spawn_tool": "self-test",
            "notes": "self-test fixture",
        }
    )
    ledger["lead"].update(
        {
            "status": "completed",
            "agent_id": "self-test-lead",
            "runtime_provenance": "self-test",
        }
    )
    ledger["fallback"].update({"status": "not-used", "reason": ""})
    for row in ledger.get("batch_workers", []):
        row.update(
            {
                "status": "completed",
                "agent_id": f"self-test-{row['batch_id']}",
                "runtime_provenance": "self-test",
                "fallback": False,
            }
        )
    if manifest.get("ui_implementation_audit", {}).get("visual_required"):
        for key in ("mockup_asset_worker", "visual_tooling_worker", "visual_comparison_worker"):
            if ledger[key].get("status") != "pending":
                continue
            ledger[key].update(
                {
                    "status": "completed",
                    "agent_id": f"self-test-{key}",
                    "runtime_provenance": "self-test",
                }
            )
    path.write_text(json.dumps(ledger, indent=2, sort_keys=True), encoding="utf-8")


def write_batch_report(out: Path, manifest: dict, batch: dict, *, out_of_scope: bool = False) -> None:
    rows = []
    inventory = []
    for unit_id in batch["coverage_units"]:
        unit = next(item for item in manifest["coverage_units"] if item["unit_id"] == unit_id)
        rows.append(f"| {unit_id} | CHECKED | {unit['sha256']} | Defines dashboard UI surface |")
        if unit["rel_path"] == "src/App.tsx":
            traces = "missing | missing | not-applicable: fixture has no authenticated role model | missing | missing"
        else:
            traces = (
                "not-applicable: static stylesheet has no event handler | "
                "not-applicable: static stylesheet has no backend call | "
                "not-applicable: static stylesheet has no permission decision | "
                "not-applicable: static stylesheet has no persistence behavior | "
                "not-applicable: rendered evidence verifies stylesheet layout separately"
            )
        inventory.append(
            f"| {unit_id} | {unit['rel_path']} | dashboard | Operations Dashboard / Resolve incident | visible label and class names | urgent incident first | source renders nav, hero, button, and archive detail | {traces} | CSS grid has desktop and mobile risk notes |"
        )
    findings = "No findings."
    if "src/App.tsx" in batch["files"]:
        findings = """- Priority: P1
- Files: src/App.tsx
- Mockup/requirement evidence: dashboard journey requires a real incident resolution path
- Interface evidence: Resolve incident button has no handler, backend, persistence, or test trace
- Expected behavior/standard: primary action should bind handler, backend result, persistence, failure states, and tests
- Gap: action trace records missing implementation and verification paths
- Suggested implementation direction: implement the real resolution workflow and cover success, permission, persistence, and failure behavior
"""
    if out_of_scope:
        findings = """- Priority: P2
- Files: src/not-owned.tsx
- Mockup/requirement evidence: dashboard mockup
- Interface evidence: out-of-scope source
- Expected behavior/standard: source owned by this batch only
- Gap: finding references a file outside this batch
- Suggested implementation direction: keep findings scoped
"""
    write(
        out / "reports" / f"{batch['id']}.md",
        f"""## Run ID
{manifest['run_id']}

## Batch ID
{batch['id']}

## Batch Summary
Dashboard UI source and styles for visual comparison.

## File Coverage
| Unit | Status | SHA-256 | Purpose |
| --- | --- | --- | --- |
{chr(10).join(rows)}

## UI Source Inventory
| Unit | File | Surface | Visible Element | Source Evidence | Expected Behavior | Actual Implementation | Handler Reference | Backend/API Reference | Permission Reference | Persistence Reference | Test Reference | Responsive/State Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
{chr(10).join(inventory)}

## Mockup And Journey Alignment
Source exposes the dashboard screen, primary action, navigation, and rare archive detail referenced by the journey docs.

## Implementation Gap Findings
{findings}

## No Gap Notes
Owned units are represented in the inventory with visible labels and layout notes.

## Open Questions
None.
""",
    )


def write_visual_evidence(out: Path, manifest: dict, *, route: str = "/dashboard") -> None:
    artifacts = out / "artifacts"
    desktop = artifacts / "desktop.png"
    desktop_full = artifacts / "desktop-full.png"
    mobile = artifacts / "mobile.png"
    mobile_full = artifacts / "mobile-full.png"
    formal = artifacts / "formal-web.json"
    journey_evidence = artifacts / "journey-evidence.json"
    review_queue = artifacts / "review-queue.json"
    manual_review = artifacts / "manual-review.json"
    write_bytes(desktop, PNG_1X1)
    write_bytes(desktop_full, PNG_1X1)
    write_bytes(mobile, PNG_1X1)
    write_bytes(mobile_full, PNG_1X1)
    desktop_sha = hashlib.sha256(desktop.read_bytes()).hexdigest()
    desktop_full_sha = hashlib.sha256(desktop_full.read_bytes()).hexdigest()
    mobile_sha = hashlib.sha256(mobile.read_bytes()).hexdigest()
    mobile_full_sha = hashlib.sha256(mobile_full.read_bytes()).hexdigest()
    source_fingerprint = "a" * 64
    intent_fingerprint = "b" * 64
    queue_payload = {
        "schemaVersion": 1,
        "kind": "formal-web-ui-review-queue",
        "runId": "formal-self-test",
        "generatedAt": "2026-07-10T00:00:00Z",
        "trigger": "declared UI inputs, journey/theme intent, or new route/state/viewport; screenshot pixels are integrity-only",
        "entries": [
            {
                "reviewCellKey": "desktop-cell",
                "sourceFingerprint": source_fingerprint,
                "intentFingerprint": intent_fingerprint,
                "screenshots": {"viewport": {"sha256": desktop_sha}, "fullPage": {"sha256": desktop_full_sha}},
            },
            {
                "reviewCellKey": "mobile-cell",
                "sourceFingerprint": source_fingerprint,
                "intentFingerprint": intent_fingerprint,
                "screenshots": {"viewport": {"sha256": mobile_sha}, "fullPage": {"sha256": mobile_full_sha}},
            },
        ],
        "carried": [],
        "removedDispositions": [],
        "undisposedRemoved": [],
        "invalidDispositions": [],
    }
    write(review_queue, json.dumps(queue_payload, indent=2))
    queue_sha = hashlib.sha256(review_queue.read_bytes()).hexdigest()
    pages = []
    for cell_id, viewport, viewport_path, viewport_sha, full_path, full_sha in (
        ("desktop-cell", {"name": "desktop", "width": 1440, "height": 900}, desktop, desktop_sha, desktop_full, desktop_full_sha),
        ("mobile-cell", {"name": "mobile", "width": 390, "height": 844}, mobile, mobile_sha, mobile_full, mobile_full_sha),
    ):
        pages.append(
            {
                "cellId": cell_id,
                "outcome": "checked",
                "metrics": {"visibleScrollbars": []},
                "findings": [],
                "screenshots": {
                    "viewport": {"path": str(viewport_path), "mime": "image/png", "sha256": viewport_sha, "width": 1, "height": 1},
                    "fullPage": {"path": str(full_path), "mime": "image/png", "sha256": full_sha, "width": 1, "height": 1},
                },
                "review": {
                    "reviewCellKey": cell_id,
                    "sourceFingerprint": source_fingerprint,
                    "intentFingerprint": intent_fingerprint,
                },
                "viewport": viewport,
                "requestedPath": route,
                "finalPath": route,
                "target": {"stateName": "base"},
            }
        )
    write(
        formal,
        json.dumps(
            {
                "schemaVersion": 2,
                "runId": "formal-self-test",
                "generatedAt": "2026-07-10T00:00:00Z",
                "browser": "chromium",
                "targets": [{"url": "http://127.0.0.1/dashboard"}],
                "pages": pages,
                "findings": [],
                "coverage": {"failed": False, "checkedPages": 2, "requiredCheckedPages": 1, "failures": [], "tolerated": []},
                "review": {
                    "queueSha256": queue_sha,
                    "pendingCount": 2,
                    "cells": [
                        {
                            "reviewCellKey": "desktop-cell",
                            "sourceFingerprint": source_fingerprint,
                            "intentFingerprint": intent_fingerprint,
                            "screenshots": pages[0]["screenshots"],
                        },
                        {
                            "reviewCellKey": "mobile-cell",
                            "sourceFingerprint": source_fingerprint,
                            "intentFingerprint": intent_fingerprint,
                            "screenshots": pages[1]["screenshots"],
                        },
                    ],
                },
            },
            indent=2,
        ),
    )
    write(
        journey_evidence,
        json.dumps(
            {
                "schemaVersion": 1,
                "kind": "formal-web-ui-journey-evidence",
                "runId": "formal-self-test",
                "governedRunId": None,
                "governedCheck": None,
                "generatedAt": "2026-07-10T00:00:00Z",
                "browser": "chromium",
                "coverage": {
                    "checkedPages": 2, "plannedPages": 2,
                    "failed": False, "readinessEligible": True,
                },
                "cells": [
                    {
                        "cellId": page["cellId"],
                        "targetName": "Dashboard",
                        "stateName": "base",
                        "outcome": "checked",
                        "screenshots": {
                            role: {
                                **page["screenshots"][role],
                                "path": Path(page["screenshots"][role]["path"]).name,
                                "size": Path(page["screenshots"][role]["path"]).stat().st_size,
                                "kind": "viewport" if role == "viewport" else "full-page",
                                "capturedAt": "2026-07-10T00:00:00Z",
                            }
                            for role in ("viewport", "fullPage")
                        },
                    }
                    for page in pages
                ],
            },
            indent=2,
        ),
    )
    formal_sha = hashlib.sha256(formal.read_bytes()).hexdigest()
    write(
        manual_review,
        json.dumps(
            {
                "schemaVersion": 1,
                "kind": "formal-web-ui-manual-review",
                "reviewedRunId": "formal-self-test",
                "reviewedAt": "2026-07-10T00:01:00Z",
                "reportSha256": formal_sha,
                "reviewQueueSha256": queue_sha,
                "decisions": [
                    {
                        "reviewCellKey": "desktop-cell",
                        "decision": "pass",
                        "note": "",
                        "sourceFingerprint": source_fingerprint,
                        "intentFingerprint": intent_fingerprint,
                        "screenshots": {"viewportSha256": desktop_sha, "fullPageSha256": desktop_full_sha},
                    },
                    {
                        "reviewCellKey": "mobile-cell",
                        "decision": "pass",
                        "note": "",
                        "sourceFingerprint": source_fingerprint,
                        "intentFingerprint": intent_fingerprint,
                        "screenshots": {"viewportSha256": mobile_sha, "fullPageSha256": mobile_full_sha},
                    },
                ],
            },
            indent=2,
        ),
    )

    write(
        out / "visual_evidence.json",
        json.dumps({"schema_version": 1, "run_id": manifest["run_id"], "artifacts": []}, indent=2),
    )
    run(
        [
            sys.executable,
            str(IMPORT_FORMAL),
            "--audit-root",
            str(out),
            "--run-id",
            manifest["run_id"],
            "--formal-report",
            str(formal),
            "--journey-evidence",
            str(journey_evidence),
            "--review-queue",
            str(review_queue),
            "--manual-review",
            str(manual_review),
        ]
    )


def formal_evidence_section() -> str:
    return """## Formal Evidence
Imported formal evidence: evidence:formal-web, evidence:formal-journey-evidence, evidence:formal-review-queue, and evidence:formal-manual-review. Coverage checked desktop and mobile states with no critical findings, no carried gaps, no visible scrollbars, and no palette risks.
"""


def write_native_visual_evidence(out: Path, manifest: dict) -> None:
    image = out / "artifacts" / "native-main.png"
    write_bytes(image, PNG_1X1)
    evidence_path = out / "visual_evidence.json"
    payload = json.loads(evidence_path.read_text(encoding="utf-8"))
    payload["artifacts"].append(
        {
            "id": "native-main",
            "kind": "native-snapshot",
            "path": image.relative_to(out).as_posix(),
            "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
            "mime": "image/png",
            "route": "Contacts",
            "state": "default native state",
            "viewport": {"width": 390, "height": 844, "label": "native mobile"},
            "captured_by": "native self-test fixture",
            "width": 1,
            "height": 1,
        }
    )
    evidence_path.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")


def native_visual_report(run_id: str) -> str:
    return f"""## Run ID
{run_id}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
The native contact journey is the visual target; no separate mockup is available.

## Visual Tooling
Native preview fixture captured evidence:native-main.

## Journey Decision Model
| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Contacts | inspect contacts | choose a contact | names and status | unavailable contact | open contact | settings | none |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| native | native mobile | choose a contact | contact list | settings | native detail navigation; badge-detail=not applicable; row-hit-target=pass; navigation-cursor=not applicable; transient-disclosure=not applicable; disclosure-scrollbar=pass; icon-meaning=pass; stable-expansion-width=not applicable; hover-copy=not applicable; status-summary=pass; message-metadata=not applicable | native snapshot evidence:native-main | PASS | native snapshot evidence:native-main |

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| native | Contact review | native mobile | Contacts | journey requirement | native preview evidence:native-main | No material mismatch in fixture | MATCHED |

## Formal Evidence
Formal Web UI verification not applicable to declared native platform.

## Findings
No findings.

## Open Questions
None.
"""


def write_blocked_formal_reports(out: Path) -> None:
    manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
    complete_ledger(out, manifest)
    for batch in manifest["batches"]:
        write_batch_report(out, manifest, batch)
    source_file = manifest["source_files"][0]["rel_path"]
    write(
        out / "reports" / "visual_comparison_audit.md",
        f"""## Run ID
{manifest['run_id']}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
Journey requirements were found; formal config is missing.

## Visual Tooling
Browser verification is blocked by the missing manifest-bound formal config.

## Journey Decision Model
| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| dashboard | review incidents | choose incident | incident list | urgent status | inspect | settings | formal selectors unresolved |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| web | desktop | blocked | blocked | blocked | badge-detail=blocked; row-hit-target=blocked; navigation-cursor=blocked; transient-disclosure=blocked; disclosure-scrollbar=blocked; icon-meaning=blocked; stable-expansion-width=blocked; hover-copy=blocked; status-summary=blocked; message-metadata=blocked | blocked by missing formal config | BLOCKED | formal verification blocked |
| web | mobile | blocked | blocked | blocked | interaction checks blocked by missing formal config | blocked by missing formal config | BLOCKED | formal verification blocked |

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| web | incident review | desktop | /dashboard | journey requirement | blocked by missing formal config | not inspected | BLOCKED |
| web | incident review | mobile | /dashboard | journey requirement | blocked by missing formal config | not inspected | BLOCKED |

## Formal Evidence
BLOCKED: manifest-bound formal config is missing.

## Findings
- Priority: P1
- Files: {source_file}
- Mockup/requirement evidence: dashboard journey requires rendered web verification
- Interface evidence: executable dashboard UI exists but has no bound formal config
- Expected behavior/standard: web audit uses a manifest-bound formal verification config
- Gap: rendered journey usability and readability evidence are blocked because formal browser verification cannot run
- Suggested implementation direction: add the project formal config and rerun the audit

## Open Questions
None.
""",
    )
    write(
        out / "final-report.md",
        f"""## Coverage
Run {manifest['run_id']} audited declared web UI platform source; formal visual coverage is blocked.

## Mockup And Requirement Inputs
Journey requirements were inventoried.

## Journey Decision Model
The dashboard incident-review decision is documented.

## Rendered Journey Usability Findings
Formal rendering is blocked by the missing config.

## Visual Audit Findings
Formal Web UI verification is blocked by the missing manifest-bound config.

## Source Implementation Findings
Source wiring references remain separately audited.

## Journey And Responsive Findings
Desktop and mobile evidence remain blocked.

## Accessibility And Interaction Findings
badge-detail=blocked; row-hit-target=blocked; navigation-cursor=blocked; transient-disclosure=blocked; disclosure-scrollbar=blocked; icon-meaning=blocked; stable-expansion-width=blocked; hover-copy=blocked; status-summary=blocked; message-metadata=blocked.

## Implementation Plan
Add the project formal config and rerun formal browser evidence.

## Verification Plan
Blocked until runtime formal verification and tests can run.
""",
    )


def write_final_report(out: Path, manifest: dict) -> None:
    platform = manifest["ui_implementation_audit"]["ui_platform"]
    if platform == "hybrid":
        visual_evidence = "evidence:formal-web, evidence:formal-journey-evidence, evidence:formal-review-queue, evidence:formal-manual-review, evidence:formal-desktop-cell-viewport, evidence:formal-mobile-cell-viewport, and evidence:native-main"
    elif platform == "web":
        visual_evidence = "evidence:formal-web, evidence:formal-journey-evidence, evidence:formal-review-queue, evidence:formal-manual-review, evidence:formal-desktop-cell-viewport, and evidence:formal-mobile-cell-viewport"
    else:
        visual_evidence = "evidence:native-main"
    write(
        out / "final-report.md",
        f"""## Coverage
Run {manifest['run_id']} audited declared {platform} UI platform source, requirements, visual evidence, and implementation wiring references.

## Mockup And Requirement Inputs
The manifest-bound mockup and journey sources were reviewed.

## Journey Decision Model
The primary journey is to review urgent incidents and decide which incident to resolve.

## Rendered Journey Usability Findings
Rendered evidence is bound through {visual_evidence}.

## Visual Audit Findings
The visual worker compared hierarchy, responsive behavior, and palette against the requirements using {visual_evidence}.

## Source Implementation Findings
Source wiring references identify missing handler, backend, persistence, and test anchors without claiming source-only outcome proof.

## Journey And Responsive Findings
Desktop, mobile, or native constraints are represented for the declared {platform} platform.

## Accessibility And Interaction Findings
badge-detail=pass; row-hit-target=pass; navigation-cursor=pass; transient-disclosure=pass; disclosure-scrollbar=pass; icon-meaning=pass; stable-expansion-width=pass; hover-copy=pass; status-summary=pass; message-metadata=pass. Evidence: {visual_evidence}.

## Implementation Plan
Implement every reported missing wiring or rendered behavior and retain formal or native evidence.

## Verification Plan
Use runtime interaction evidence and focused tests for observable outcomes; source wiring references alone are not outcome proof.
""",
    )


def write_complete_reports(
    out: Path,
    *,
    out_of_scope: bool = False,
    weak_visual_evidence: bool = False,
    real_visual_evidence: bool = True,
) -> dict:
    manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
    complete_ledger(out, manifest)
    for batch in manifest["batches"]:
        write_batch_report(out, manifest, batch, out_of_scope=out_of_scope)
    if manifest.get("ui_implementation_audit", {}).get("visual_required"):
        platform = manifest["ui_implementation_audit"]["ui_platform"]
        if real_visual_evidence:
            if platform in {"web", "hybrid"}:
                write_visual_evidence(out, manifest)
            if platform in {"native", "hybrid"}:
                write_native_visual_evidence(out, manifest)
        source_file = manifest["source_files"][0]["rel_path"]
        if manifest["ui_implementation_audit"].get("mockup_asset_report"):
            write(
                out / "reports" / "mockup_asset_audit.md",
            f"""## Run ID
{manifest['run_id']}

## Worker
mockup_asset_audit

## Mockup/Asset Inputs
design/mockups/dashboard-mobile.png was visually inspected as a dashboard mockup.

## Journey Requirement Inputs
docs/journeys.md defines the dashboard journey and mobile hierarchy.

## Expected Screens And Visual Requirements
Dashboard should show urgent incident summary and Resolve incident before archive details on desktop and mobile.

## Findings
No findings.

## Open Questions
None.
""",
            )
        if manifest["ui_implementation_audit"].get("visual_tooling_report"):
            write(
                out / "reports" / "visual_tooling_audit.md",
            f"""## Run ID
{manifest['run_id']}

## Worker
visual_tooling_audit

## Tooling Inventory
package.json exposes vite dev and Playwright visual test scripts.

## Safe Run Path
Run npm scripts in fixture mode and open the dashboard route locally.

## Desktop/Mobile Screenshot Plan
Capture desktop 1440px and mobile 390px screenshots for the dashboard route.

## Findings
No findings.

## Open Questions
None.
""",
            )
        if platform == "native":
            write(out / "reports" / "visual_comparison_audit.md", native_visual_report(manifest["run_id"]))
            write_final_report(out, manifest)
            return manifest
        native_usability = (
            "| native | native mobile | choose a contact | contact list | settings | native detail navigation | native snapshot evidence:native-main | PASS | native snapshot evidence:native-main |"
            if platform == "hybrid"
            else ""
        )
        native_comparison = (
            "| native | Contact review | native mobile | Contacts | journey requirement | native preview evidence:native-main | No material mismatch in fixture | MATCHED |"
            if platform == "hybrid"
            else ""
        )
        evidence = "looked fine" if weak_visual_evidence else "playwright capture with formal verifier evidence:formal-web"
        write(
            out / "reports" / "visual_comparison_audit.md",
            f"""## Run ID
{manifest['run_id']}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
design/mockups/dashboard-mobile.png and docs/journeys.md define the expected dashboard hierarchy.

## Visual Tooling
Playwright captures desktop 1440px and mobile 390px views; formal browser verification records geometry.

## Journey Decision Model
| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| dashboard | review urgent incidents first | decide which incident to resolve | active incident summary and severity | urgent severity and stale status | resolve incident | navigation to reports and archive export details | none |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| web | desktop | decide which incident to resolve | navigation and active incident hero | archive detail | inline secondary aside | playwright screenshot evidence:formal-desktop-cell-viewport and DOM viewport measurement | PASS | playwright screenshot evidence:formal-desktop-cell-viewport and DOM viewport measurement 12% controls |
| web | mobile | decide which incident to resolve | active incident summary and Resolve incident action | archive detail | secondary content after primary action | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement | PASS | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement 10% controls |
{native_usability}

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| web | Dashboard review | desktop | /dashboard | design/mockups/dashboard-mobile.png and docs/journeys.md | {evidence} evidence:formal-desktop-cell-viewport | No material desktop mismatch in fixture report | MATCHED |
| web | Dashboard review | mobile | /dashboard | design/mockups/dashboard-mobile.png and docs/journeys.md | {evidence} evidence:formal-mobile-cell-viewport | No material mobile mismatch in fixture report | MATCHED |
{native_comparison}

{INTERACTION_CHECKLIST_LINE}

{formal_evidence_section()}

## Findings
No findings.

## Open Questions
None.
""",
        )
    write_final_report(out, manifest)
    return manifest


def write_currency_priority_visual_report(out: Path, *, include_p1: bool) -> None:
    manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
    write_visual_evidence(out, manifest, route="/currency-rates")
    source_file = next(item["rel_path"] for item in manifest["source_files"] if item["rel_path"].endswith(".tsx"))
    first_viewport_result = "GAP" if include_p1 else "PASS"
    visual_result = "GAP" if include_p1 else "MATCHED"
    findings = "No findings."
    if include_p1:
        findings = f"""- Priority: P1
- Files: {source_file}
- Mockup/requirement evidence: docs/currency-rates-journey.md requires most-used live rates before target/settings controls on mobile.
- Interface evidence: mobile screenshot currency-rates-mobile.png and DOM viewport measurement show Target Currency settings dominate the visible surface while Most-used rates are buried below secondary controls.
- Expected behavior/standard: rendered journey surface should let users decide current rates without secondary target settings overwhelming that decision path.
- Gap: the target/settings block dominates the visible surface and buries most-used rates.
- Suggested implementation direction: make most-used live rates the dominant decision-driving content and move target settings into a secondary detail path.
"""
    write(
        out / "reports" / "visual_comparison_audit.md",
        f"""## Run ID
{manifest['run_id']}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
docs/journeys.md defines the required first-viewport priority.

## Visual Tooling
Playwright desktop and mobile captures with formal geometry evidence.

## Journey Decision Model
| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| currency rates | decide current most-used live rates quickly | decide current exchange rates | most-used live rates list | stale rate warning | inspect rates | target currency adjustment and target/settings configuration | none |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| web | desktop | decide current rates | rates chart and most-used rates | target settings | inline secondary panel | playwright screenshot evidence:formal-desktop-cell-viewport and DOM viewport measurement | PASS | playwright screenshot evidence:formal-desktop-cell-viewport and DOM viewport measurement 18% secondary controls |
| web | mobile | only target currency configuration is supported; rate decision is buried | Target Currency settings form and Apply settings button | target/settings controls dominate while most-used rates are buried under duplicate summaries and vague labels | secondary controls dominate visible surface | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement | {first_viewport_result} | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement 82% controls before rates |

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| web | Currency rates decision | desktop | /currency-rates | docs/currency-rates-journey.md | playwright screenshot evidence:formal-desktop-cell-viewport and formal evidence:formal-web | Desktop still exposes primary rates | MATCHED |
| web | Currency rates decision | mobile | /currency-rates | docs/currency-rates-journey.md | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement | Target Currency block dominates the visible surface and buries most-used rates | {visual_result} |

{INTERACTION_CHECKLIST_LINE}

{formal_evidence_section()}

## Findings
{findings}

## Open Questions
None.
""",
    )


def write_layout_noise_visual_report(out: Path, *, include_p1: bool) -> None:
    manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
    write_visual_evidence(out, manifest, route="/review")
    source_file = manifest["source_files"][0]["rel_path"]
    result = "GAP" if include_p1 else "MATCHED"
    usability_result = "GAP" if include_p1 else "PASS"
    findings = "No findings."
    if include_p1:
        findings = f"""- Priority: P1
- Files: {source_file}
- Mockup/requirement evidence: docs/journeys.md requires the review workspace to make the next-case decision quickly from primary facts.
- Interface evidence: desktop screenshot review-workspace-desktop.png shows nested blocks inside blocks, border stacks, visual noise, weak grid alignment, an unstable disclosure that changes width, a row that is not clickable unless a tiny icon-only target is hit, a disclosure icon that overlaps the scrollbar, flags with no hover/click popover feedback, selectable timestamps, permanent helper text, unintuitive icons, avatar clutter, and left/right message alignment problems.
- Expected behavior/standard: rendered UI should make critical decision information prominent, keep secondary detail reachable without dominating, and use stable aligned whole-row disclosure controls, interactive badges with useful popover detail, meaningful icons, passive metadata, and quiet message layout.
- Gap: the noisy frame stack and unstable disclosure obscure the decision hierarchy and make lower-importance detail look as important as critical decision content.
- Suggested implementation direction: flatten nested surfaces, normalize grid gutters, stabilize disclosure width and control position, move obvious instructions to hints, replace decorative/meaningless icons, and align message groups by sender.
"""
    write(
        out / "reports" / "visual_comparison_audit.md",
        f"""## Run ID
{manifest['run_id']}

## Worker
visual_comparison_audit

## Mockup And Asset Inventory
docs/journeys.md defines the review-workspace decision hierarchy.

## Visual Tooling
Playwright desktop and mobile captures with formal geometry evidence.

## Journey Decision Model
| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| review workspace | choose the next case to resolve | decide which case needs action now | case status, urgency, and owner summary | blocked or stale case state | open case | raw metadata and diagnostic history | none |

## Rendered Journey Usability
| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| web | desktop | next-case decision is hard to scan | decision-critical facts are weakly placed inside nested blocks inside blocks | low-importance raw metadata, sender labels, selectable timestamps, permanent instruction noise, avatar clutter, and helper text dominate | unstable expander jumps horizontally, width changes, row is not clickable except a tiny icon-only target, and the disclosure icon interferes with the scrollbar | playwright screenshot evidence:formal-desktop-cell-viewport and DOM viewport measurement | {usability_result} | playwright screenshot evidence:formal-desktop-cell-viewport and DOM viewport measurement |
| web | mobile | next-case decision is hard to scan | decision-critical facts are buried below noisy surfaces | secondary detail and decorative clutter dominate | flags have no hover feedback and no popover detail, while expanded and collapsed result blocks have different widths | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement | {usability_result} | playwright screenshot evidence:formal-mobile-cell-viewport and DOM viewport measurement |

## Visual Comparison Checks
| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| web | Review workspace | desktop | /review | docs/journeys.md | playwright screenshot evidence:formal-desktop-cell-viewport and formal evidence:formal-web | nested cards, border stacks, visual noise, misalignment, unintuitive icons, permanent instruction helper text, avatar clutter, icon-only row activation, expander/scrollbar collision, and unstable disclosure width changes | {result} |
| web | Review workspace | mobile | /review | docs/journeys.md | playwright screenshot evidence:formal-mobile-cell-viewport | weak grid, badge no hover/click popover detail, low-importance detail dominates, sender labels and selectable timestamps add noise, and message alignment problems hide the decision hierarchy | {result} |

{INTERACTION_CHECKLIST_LINE}

{formal_evidence_section()}

## Findings
{findings}

## Open Questions
None.
""",
    )


def assert_ledger_mutation_fails(out: Path, tmp: Path, name: str, mutate) -> None:
    mutated = tmp / name
    shutil.copytree(out, mutated)
    ledger_path = mutated / "execution_ledger.json"
    ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    mutate(ledger)
    ledger_path.write_text(json.dumps(ledger, indent=2, sort_keys=True), encoding="utf-8")
    result = verify(mutated, expect=1)
    check("execution_ledger_issues" in result.stdout, f"{name} should fail execution ledger verification")


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="ui-implementation-audit-self-test-"))
    try:
        ui_fixture = tmp / "ui-fixture"
        make_ui_fixture(ui_fixture)
        out = tmp / "out"
        manifest = build(ui_fixture, out)
        check(manifest["audit_kind"] == "ui-implementation", "manifest should record ui-implementation audit kind")
        check(manifest["logs_dir"] == str((out / "logs").resolve()), "manifest should record cold logs_dir")
        check(manifest["final_report"] == str((out / "final-report.md").resolve()), "manifest should record final report path")
        check((out / "logs").is_dir(), "builder should create cold logs directory")
        marker = json.loads((out / ".ui-implementation-audit-artifacts.json").read_text(encoding="utf-8"))
        check("logs" in marker["generated_artifacts"] and "final-report.md" in marker["generated_artifacts"], "ownership marker should reserve cold logs and final report")
        for batch in manifest["batches"]:
            prompt_text = (out / batch["prompt"]).read_text(encoding="utf-8")
            check(str((out / batch["report"]).resolve()) in prompt_text, "batch prompt should contain exact report path")
            for token in (
                "fresh isolated context",
                "runtime/user-selected worker default",
                "REPORT_SAVED",
                f"filename={Path(batch['report']).name};",
                "REPORT_NOT_SAVED path=<exact-report-path>; reason=<short-reason>",
                "at or below 80 tokens",
            ):
                check(token in prompt_text, f"batch prompt should enforce context-light artifact delivery: {token}")
            check("## Journey Decision Model" not in prompt_text, "source workers must not duplicate the lead-owned journey model")
            check("## Rendered Journey Usability" not in prompt_text, "source workers must not duplicate rendered visual judgment")
            check("Interaction checklist:" not in prompt_text, "source workers must not duplicate the visual interaction checklist")
        audit_meta = manifest["ui_implementation_audit"]
        check(audit_meta["ui_platform"] == "web", "default fixture should declare web platform")
        check(audit_meta["formal_config"]["rel_path"] == "formal-web-ui.json", "web fixture should bind its formal config")
        check(audit_meta["visual_worker_mode"] == "combined", "default visual audit should use one combined worker")
        check(not audit_meta["mockup_asset_prompt"] and not audit_meta["visual_tooling_prompt"], "default visual audit should not create redundant discovery workers")
        for prompt_key, report_key in (
            ("mockup_asset_prompt", "mockup_asset_report"),
            ("visual_tooling_prompt", "visual_tooling_report"),
            ("visual_comparison_prompt", "visual_comparison_report"),
        ):
            if not audit_meta[prompt_key]:
                continue
            prompt_text = (out / audit_meta[prompt_key]).read_text(encoding="utf-8")
            check(str((out / audit_meta[report_key]).resolve()) in prompt_text, f"{prompt_key} should contain exact report path")
            check(
                "fresh isolated context" in prompt_text and "REPORT_SAVED" in prompt_text,
                f"{prompt_key} should use a compact isolated-worker contract",
            )
            check(
                f"filename={Path(audit_meta[report_key]).name};" in prompt_text,
                f"{prompt_key} receipt should name its saved report filename",
            )
        visual_prompt_text = (out / audit_meta["visual_comparison_prompt"]).read_text(encoding="utf-8")
        check(str((out / "visual_evidence.json").resolve()) in visual_prompt_text, "visual worker should receive exact evidence-manifest path")
        check(
            audit_meta["formal_config"]["rel_path"] in visual_prompt_text
            and audit_meta["formal_config"]["sha256"] in visual_prompt_text,
            "visual worker should receive the manifest-bound formal config path and hash",
        )
        for token in (
            "review-queue.json",
            "formal_web_ui_review.py",
            "open only each queued cell",
            "initial-viewport and full-page",
            "review-queue",
            "manual-review",
            "Formal Evidence",
            "import_formal_web_evidence.py",
        ):
            check(token in visual_prompt_text, f"visual worker prompt should enforce changed-review behavior: {token}")
        check("## Journey Decision Model" in visual_prompt_text and "## Rendered Journey Usability" in visual_prompt_text, "visual worker must own journey and rendered analysis")
        skill_contract = (ROOT / "SKILL.md").read_text(encoding="utf-8")
        folded_skill_contract = " ".join(skill_contract.split()).casefold()
        skill_frontmatter = skill_contract.split("---", 2)[1]
        skill_adapter = (ROOT / "agents" / "openai.yaml").read_text(encoding="utf-8")
        check(
            "this is an explicit-only" in folded_skill_contract
            and "active runtime's explicit skill-invocation mechanism"
            in folded_skill_contract,
            "ui-implementation-audit contract must require runtime-neutral explicit invocation",
        )
        check(
            "do not use for ordinary implementation or review"
            in folded_skill_contract,
            "ordinary UI work must not trigger ui-implementation-audit implicitly",
        )
        check(
            "allow_implicit_invocation: false" in skill_adapter
            and "description:" in skill_frontmatter,
            "ui-implementation-audit adapter must disable implicit invocation",
        )
        check("fresh isolated workers" in skill_contract, "skill must require isolated workers")
        check('reasoning_effort="low"' not in skill_contract and "Light-effort" not in skill_contract, "skill must not prescribe worker effort")
        for forbidden in ("Codex", "Claude", "fork_turns"):
            check(forbidden not in skill_contract, f"shared skill must remain neutral: {forbidden}")
        for active_path in (
            ROOT / "SKILL.md",
            ROOT / "README.md",
            ROOT / "agents" / "openai.yaml",
            ROOT / "scripts" / "build_ui_implementation_audit_batches.py",
        ):
            active_text = active_path.read_text(encoding="utf-8")
            for forbidden in ('reasoning_effort="low"', "Light-effort", "required_reasoning_effort", "actual_reasoning_effort", "can_set_reasoning_effort"):
                check(forbidden not in active_text, f"active UI-audit contract must not prescribe worker effort: {active_path} contains {forbidden}")
        check(
            "final-report.md" in skill_contract and "filename-bearing `REPORT_SAVED`" in skill_contract,
            "skill must keep complete results in cold artifacts with filename-bearing receipts",
        )
        for token in ("journey-evidence", "review-queue.json", "formal_web_ui_review.py", "formal-evidence chain", "import_formal_web_evidence.py"):
            check(token in skill_contract, f"skill contract should preserve changed-review rule: {token}")
        agent_metadata = (ROOT / "agents" / "openai.yaml").read_text(encoding="utf-8")
        check(
            "\npolicy:\n  allow_implicit_invocation: false\n" in f"\n{agent_metadata}",
            "ui-implementation-audit metadata must disable implicit invocation",
        )
        check(
            any(
                line.strip().startswith("default_prompt:") and "$ui-implementation-audit" in line
                for line in agent_metadata.splitlines()
            ),
            "ui-implementation-audit default prompt must explicitly invoke $ui-implementation-audit",
        )
        check(manifest["source_file_count"] == 2, "only interface source files should be queued")
        queued = {item["rel_path"] for item in manifest["source_files"]}
        check(queued == {"src/App.tsx", "src/styles.css"}, f"unexpected source queue: {queued}")
        check(manifest["ui_implementation_audit"]["mockup_asset_count"] >= 1, "mockup asset should be discovered")
        check(manifest["ui_implementation_audit"]["requirement_source_count"] >= 1, "journey requirement should be discovered")
        split_out = tmp / "split-visual-out"
        split_manifest = build(ui_fixture, split_out, "--split-visual-discovery")
        split_meta = split_manifest["ui_implementation_audit"]
        check(split_meta["visual_worker_mode"] == "split", "explicit split mode should be recorded")
        check(split_meta["mockup_asset_prompt"] and split_meta["visual_tooling_prompt"], "explicit split mode should create both discovery prompts")
        implementation_gate = manifest["ui_implementation_audit"]["implementation_gate"]
        check(implementation_gate["status"] == "passed", "implemented UI gate should pass")
        check(
            [item["rel_path"] for item in implementation_gate["evidence_files"]] == ["src/App.tsx"],
            "gate should bind the explicitly inspected product UI file",
        )
        check(
            implementation_gate["evidence_files"][0]["sha256"]
            == next(item["sha256"] for item in manifest["source_files"] if item["rel_path"] == "src/App.tsx"),
            "gate evidence hash should bind to source manifest hash",
        )

        no_evidence_out = tmp / "no-evidence-out"
        no_evidence = assert_inapplicable(ui_fixture, no_evidence_out)
        check("not applicable" in no_evidence.stderr.lower(), "missing implementation evidence should be explicitly inapplicable")

        eligibility_out = tmp / "eligibility-only-out"
        eligibility = run(
            [
                sys.executable,
                str(BUILD),
                "--repo",
                str(ui_fixture),
                "--out",
                str(eligibility_out),
                "--implemented-ui-file",
                "src/App.tsx",
                "--eligibility-only",
            ]
        )
        eligibility_payload = json.loads(eligibility.stdout)
        check(eligibility_payload["status"] == "passed", "eligibility-only preflight should pass real UI")
        check(not eligibility_out.exists(), "eligibility-only preflight must not create audit artifacts")

        missing_platform_out = tmp / "missing-platform-out"
        missing_platform = run(
            [
                sys.executable,
                str(BUILD),
                "--repo",
                str(ui_fixture),
                "--out",
                str(missing_platform_out),
                "--implemented-ui-file",
                "src/App.tsx",
            ],
            expect=2,
        )
        check("--ui-platform is required" in missing_platform.stderr, "full audits must declare platform explicitly")

        missing_formal_out = tmp / "missing-formal-config-out"
        run(
            [
                sys.executable,
                str(BUILD),
                "--repo",
                str(ui_fixture),
                "--out",
                str(missing_formal_out),
                "--implemented-ui-file",
                "src/App.tsx",
                "--ui-platform",
                "web",
            ]
        )
        missing_formal_manifest = json.loads((missing_formal_out / "manifest.json").read_text(encoding="utf-8"))
        check(missing_formal_manifest["ui_implementation_audit"]["formal_config"] is None, "missing formal config should remain explicit in the manifest")
        check("missing" in (missing_formal_out / "visual_comparison_audit.md").read_text(encoding="utf-8"), "visual prompt should preserve missing formal config blocker")
        write_blocked_formal_reports(missing_formal_out)
        check("ok: true" in verify(missing_formal_out).stdout, "a complete blocked audit should verify without fabricating formal evidence")

        native_fixture = tmp / "native-ui-fixture"
        make_native_ui_fixture(native_fixture)
        native_out = tmp / "native-out"
        native_manifest = build(native_fixture, native_out, platform="native")
        check(native_manifest["ui_implementation_audit"]["ui_platform"] == "native", "native platform should be explicit")
        check(native_manifest["ui_implementation_audit"]["formal_config"] is None, "native audit must not bind formal web config")
        write_complete_reports(native_out)
        check("ok: true" in verify(native_out).stdout, "native audit should verify with native snapshot and no formal web evidence")

        hybrid_out = tmp / "hybrid-out"
        hybrid_manifest = build(ui_fixture, hybrid_out, platform="hybrid")
        check(hybrid_manifest["ui_implementation_audit"]["ui_platform"] == "hybrid", "hybrid platform should be explicit")
        write_complete_reports(hybrid_out)
        check("ok: true" in verify(hybrid_out).stdout, "hybrid audit should require and accept both formal web and native evidence")

        precision_fixture = tmp / "react-native-platform-fixture"
        make_gate_precision_fixture(precision_fixture)
        react_native_out = tmp / "react-native-platform-out"
        react_native_manifest = build(
            precision_fixture,
            react_native_out,
            evidence="src/NativeContacts.tsx",
            platform="native",
        )
        check(react_native_manifest["ui_implementation_audit"]["ui_platform"] == "native", "React Native TSX must not be classified as browser UI by extension")
        check(react_native_manifest["ui_implementation_audit"]["formal_config"] is None, "React Native audit must not require formal web evidence")
        missing_visual_artifacts_out = tmp / "missing-visual-artifacts-out"
        build(ui_fixture, missing_visual_artifacts_out)
        write_complete_reports(missing_visual_artifacts_out, real_visual_evidence=False)
        missing_visual_artifacts_result = verify(missing_visual_artifacts_out, expect=1)
        check("visual evidence" in missing_visual_artifacts_result.stdout.lower(), "named but nonexistent screenshots must fail verification")
        write_complete_reports(out)
        result = verify(out)
        check("ok: true" in result.stdout, "complete report should verify")
        importer_repeat = run(
            [
                sys.executable,
                str(IMPORT_FORMAL),
                "--audit-root",
                str(out),
                "--run-id",
                manifest["run_id"],
                "--formal-report",
                str(out / "artifacts" / "formal-web.json"),
                "--journey-evidence",
                str(out / "artifacts" / "journey-evidence.json"),
                "--review-queue",
                str(out / "artifacts" / "review-queue.json"),
                "--manual-review",
                str(out / "artifacts" / "manual-review.json"),
            ]
        )
        check(json.loads(importer_repeat.stdout)["ok"], "formal evidence importer should be idempotent for identical evidence")

        importer_tamper_out = tmp / "importer-tamper-out"
        shutil.copytree(out, importer_tamper_out)
        tampered_manifest_path = importer_tamper_out / "visual_evidence.json"
        tampered_manifest = json.loads(tampered_manifest_path.read_text(encoding="utf-8"))
        tampered_manifest["artifacts"] = []
        write(tampered_manifest_path, json.dumps(tampered_manifest, indent=2))
        copied_formal_path = importer_tamper_out / "artifacts" / "formal-web.json"
        copied_formal = json.loads(copied_formal_path.read_text(encoding="utf-8"))
        for page in copied_formal["pages"]:
            for role in ("viewport", "fullPage"):
                original_name = Path(page["screenshots"][role]["path"]).name
                page["screenshots"][role]["path"] = str(importer_tamper_out / "artifacts" / original_name)
        write(copied_formal_path, json.dumps(copied_formal, indent=2))
        copied_review_path = importer_tamper_out / "artifacts" / "manual-review.json"
        copied_review = json.loads(copied_review_path.read_text(encoding="utf-8"))
        copied_review["reportSha256"] = hashlib.sha256(copied_formal_path.read_bytes()).hexdigest()
        write(copied_review_path, json.dumps(copied_review, indent=2))
        write_bytes(importer_tamper_out / "artifacts" / "desktop.png", PNG_1X1 + b"tampered")
        importer_tamper = run(
            [
                sys.executable,
                str(IMPORT_FORMAL),
                "--audit-root",
                str(importer_tamper_out),
                "--run-id",
                manifest["run_id"],
                "--formal-report",
                str(importer_tamper_out / "artifacts" / "formal-web.json"),
                "--journey-evidence",
                str(importer_tamper_out / "artifacts" / "journey-evidence.json"),
                "--review-queue",
                str(importer_tamper_out / "artifacts" / "review-queue.json"),
                "--manual-review",
                str(importer_tamper_out / "artifacts" / "manual-review.json"),
            ],
            expect=2,
        )
        check("hash does not match" in json.loads(importer_tamper.stdout)["error"], "formal evidence importer must reject tampered screenshots")

        missing_final_out = tmp / "missing-final-report-out"
        shutil.copytree(out, missing_final_out)
        (missing_final_out / "final-report.md").unlink()
        missing_final_result = verify(missing_final_out, expect=1)
        check("final_report_issues" in missing_final_result.stdout and "missing" in missing_final_result.stdout, "missing final report must fail completion")

        malformed_final_out = tmp / "malformed-final-report-out"
        shutil.copytree(out, malformed_final_out)
        malformed_final_path = malformed_final_out / "final-report.md"
        malformed_final_path.write_text(
            malformed_final_path.read_text(encoding="utf-8").replace("## Verification Plan", "## Unverified Plan"),
            encoding="utf-8",
        )
        malformed_final_result = verify(malformed_final_out, expect=1)
        check("final report sections must match required order" in malformed_final_result.stdout, "malformed final report headings must fail completion")

        overclaimed_final_out = tmp / "overclaimed-final-report-out"
        shutil.copytree(out, overclaimed_final_out)
        overclaimed_final_path = overclaimed_final_out / "final-report.md"
        overclaimed_final_path.write_text(
            overclaimed_final_path.read_text(encoding="utf-8").replace(
                "source wiring references alone are not outcome proof",
                "path#symbol proves observable success",
            ),
            encoding="utf-8",
        )
        overclaimed_final_result = verify(overclaimed_final_out, expect=1)
        check("source wiring references must not be represented as observable outcome proof" in overclaimed_final_result.stdout, "source-only wiring must not be overclaimed as outcome proof")

        legacy_gate_out = tmp / "legacy-recognized-gate-out"
        shutil.copytree(out, legacy_gate_out)
        legacy_gate_manifest_path = legacy_gate_out / "manifest.json"
        legacy_gate_manifest = json.loads(legacy_gate_manifest_path.read_text(encoding="utf-8"))
        legacy_gate = legacy_gate_manifest["ui_implementation_audit"]["implementation_gate"]
        legacy_gate["schema_version"] = 1
        for item in legacy_gate["evidence_files"]:
            item.pop("qualification", None)
        write(legacy_gate_manifest_path, json.dumps(legacy_gate_manifest, indent=2))
        legacy_gate_result = verify(legacy_gate_out)
        check(
            "ok: true" in legacy_gate_result.stdout,
            "legacy gate evidence should remain verifiable only when the current strict detector recognizes it",
        )

        missing_gate_out = tmp / "missing-gate-out"
        shutil.copytree(out, missing_gate_out)
        missing_gate_manifest_path = missing_gate_out / "manifest.json"
        missing_gate_manifest = json.loads(missing_gate_manifest_path.read_text(encoding="utf-8"))
        missing_gate_manifest["ui_implementation_audit"]["implementation_gate"]["evidence_files"] = []
        write(missing_gate_manifest_path, json.dumps(missing_gate_manifest, indent=2))
        missing_gate_result = verify(missing_gate_out, expect=2)
        check("implementation_gate" in missing_gate_result.stderr, "empty implementation gate must fail manifest verification")

        forged_gate_out = tmp / "forged-gate-out"
        shutil.copytree(out, forged_gate_out)
        forged_gate_manifest_path = forged_gate_out / "manifest.json"
        forged_gate_manifest = json.loads(forged_gate_manifest_path.read_text(encoding="utf-8"))
        css_source = next(item for item in forged_gate_manifest["source_files"] if item["rel_path"] == "src/styles.css")
        forged_gate_manifest["ui_implementation_audit"]["implementation_gate"]["evidence_files"] = [
            {
                "rel_path": css_source["rel_path"],
                "sha256": css_source["sha256"],
                "evidence": "forged manifest evidence",
                "qualification": {"method": "recognized-ui-signal", "detector_version": 2},
            }
        ]
        write(forged_gate_manifest_path, json.dumps(forged_gate_manifest, indent=2))
        forged_gate_result = verify(forged_gate_out, expect=2)
        check(
            "not a qualifying product UI source" in forged_gate_result.stderr,
            "verifier must rerun the same implementation evidence predicate",
        )

        legacy_forged_gate_out = tmp / "legacy-forged-gate-out"
        shutil.copytree(out, legacy_forged_gate_out)
        legacy_forged_manifest_path = legacy_forged_gate_out / "manifest.json"
        legacy_forged_manifest = json.loads(legacy_forged_manifest_path.read_text(encoding="utf-8"))
        legacy_forged_manifest["ui_implementation_audit"]["implementation_gate"].update(
            {
                "schema_version": 1,
                "evidence_files": [
                    {
                        "rel_path": css_source["rel_path"],
                        "sha256": css_source["sha256"],
                        "evidence": "legacy forged manifest evidence",
                    }
                ],
            }
        )
        write(legacy_forged_manifest_path, json.dumps(legacy_forged_manifest, indent=2))
        legacy_forged_result = verify(legacy_forged_gate_out, expect=2)
        check(
            "not a qualifying product UI source" in legacy_forged_result.stderr,
            "legacy gates must not preserve permissive backend or non-UI evidence",
        )

        tampered_visual_out = tmp / "tampered-visual-out"
        shutil.copytree(out, tampered_visual_out)
        write_bytes(tampered_visual_out / "artifacts" / "desktop.png", PNG_1X1 + b"tampered")
        tampered_visual_result = verify(tampered_visual_out, expect=1)
        check("sha256" in tampered_visual_result.stdout, "tampered screenshot bytes must fail evidence hash verification")

        wrong_metadata_out = tmp / "wrong-visual-metadata-out"
        shutil.copytree(out, wrong_metadata_out)
        wrong_metadata_path = wrong_metadata_out / "visual_evidence.json"
        wrong_metadata = json.loads(wrong_metadata_path.read_text(encoding="utf-8"))
        next(item for item in wrong_metadata["artifacts"] if item["id"] == "formal-desktop-cell-viewport")["route"] = "/invented-route"
        write(wrong_metadata_path, json.dumps(wrong_metadata, indent=2))
        wrong_metadata_result = verify(wrong_metadata_out, expect=1)
        check("route metadata does not match" in wrong_metadata_result.stdout, "screenshot route metadata must bind to the report row")

        weak_formal_out = tmp / "weak-formal-out"
        shutil.copytree(out, weak_formal_out)
        formal_path = weak_formal_out / "artifacts" / "formal-web.json"
        formal_payload = json.loads(formal_path.read_text(encoding="utf-8"))
        formal_payload["pages"][0]["metrics"].pop("visibleScrollbars")
        write(formal_path, json.dumps(formal_payload, indent=2))
        weak_formal_manifest_path = weak_formal_out / "visual_evidence.json"
        weak_formal_manifest = json.loads(weak_formal_manifest_path.read_text(encoding="utf-8"))
        next(item for item in weak_formal_manifest["artifacts"] if item["id"] == "formal-web")["sha256"] = hashlib.sha256(formal_path.read_bytes()).hexdigest()
        write(weak_formal_manifest_path, json.dumps(weak_formal_manifest, indent=2))
        weak_formal_result = verify(weak_formal_out, expect=1)
        check("visibleScrollbars" in weak_formal_result.stdout, "formal verifier JSON must preserve visible scrollbar inventory")

        missing_review_chain_out = tmp / "missing-review-chain-out"
        shutil.copytree(out, missing_review_chain_out)
        missing_review_manifest_path = missing_review_chain_out / "visual_evidence.json"
        missing_review_manifest = json.loads(missing_review_manifest_path.read_text(encoding="utf-8"))
        missing_review_manifest["artifacts"] = [
            item for item in missing_review_manifest["artifacts"] if item["id"] != "formal-manual-review"
        ]
        write(missing_review_manifest_path, json.dumps(missing_review_manifest, indent=2))
        missing_review_chain_result = verify(missing_review_chain_out, expect=1)
        check("manual-review" in missing_review_chain_result.stdout, "rendered web UI must bind finalized manual-review evidence")

        forged_review_chain_out = tmp / "forged-review-chain-out"
        shutil.copytree(out, forged_review_chain_out)
        forged_review_path = forged_review_chain_out / "artifacts" / "manual-review.json"
        forged_review = json.loads(forged_review_path.read_text(encoding="utf-8"))
        forged_review["reportSha256"] = "f" * 64
        write(forged_review_path, json.dumps(forged_review, indent=2))
        forged_review_manifest_path = forged_review_chain_out / "visual_evidence.json"
        forged_review_manifest = json.loads(forged_review_manifest_path.read_text(encoding="utf-8"))
        next(item for item in forged_review_manifest["artifacts"] if item["id"] == "formal-manual-review")["sha256"] = hashlib.sha256(forged_review_path.read_bytes()).hexdigest()
        write(forged_review_manifest_path, json.dumps(forged_review_manifest, indent=2))
        forged_review_chain_result = verify(forged_review_chain_out, expect=1)
        check("does not bind a registered formal report" in forged_review_chain_result.stdout, "manual review must bind the registered formal report bytes")

        invented_wiring_out = tmp / "invented-wiring-out"
        shutil.copytree(out, invented_wiring_out)
        action_report = next((invented_wiring_out / "reports").glob("batch_*.md"))
        action_report.write_text(
            action_report.read_text(encoding="utf-8").replace(
                "| missing | missing | not-applicable: fixture has no authenticated role model |",
                "| src/App.tsx#inventedHandler | missing | not-applicable: fixture has no authenticated role model |",
                1,
            ),
            encoding="utf-8",
        )
        invented_action_result = verify(invented_wiring_out, expect=1)
        check("symbol/text is absent" in invented_action_result.stdout, "invented handler symbols must fail wiring-reference verification")

        missing_trace_without_finding_out = tmp / "missing-trace-without-finding-out"
        shutil.copytree(out, missing_trace_without_finding_out)
        missing_trace_report = next((missing_trace_without_finding_out / "reports").glob("batch_*.md"))
        missing_trace_report.write_text(
            re.sub(
                r"(?s)(## Implementation Gap Findings\n).*?\n\n## No Gap Notes",
                r"\1No findings.\n\n## No Gap Notes",
                missing_trace_report.read_text(encoding="utf-8"),
            ),
            encoding="utf-8",
        )
        missing_trace_result = verify(missing_trace_without_finding_out, expect=1)
        check("missing handler/backend/permission/persistence/test wiring references require a finding" in missing_trace_result.stdout, "missing wiring references must not pass under No findings")

        checklist_missing_out = tmp / "checklist-missing-out"
        shutil.copytree(out, checklist_missing_out)
        checklist_report = checklist_missing_out / "reports" / "visual_comparison_audit.md"
        checklist_report.write_text(
            checklist_report.read_text(encoding="utf-8").replace(INTERACTION_CHECKLIST_LINE, "Interaction checklist: omitted."),
            encoding="utf-8",
        )
        checklist_missing_result = verify(checklist_missing_out, expect=1)
        check(
            "interaction checklist label" in checklist_missing_result.stdout,
            "visual comparison report missing interaction checklist labels should fail verification",
        )

        missing_report_out = tmp / "missing-report-out"
        shutil.copytree(out, missing_report_out)
        first_report = next((missing_report_out / "reports").glob("batch_*.md"))
        first_report.unlink()
        missing_result = verify(missing_report_out, expect=1)
        check("missing_reports" in missing_result.stdout, "missing batch report should fail verification")

        weak_visual_out = tmp / "weak-visual-out"
        build(ui_fixture, weak_visual_out)
        write_complete_reports(weak_visual_out, weak_visual_evidence=True)
        weak_visual_result = verify(weak_visual_out, expect=1)
        check("Implementation Screenshot/Tool Evidence" in weak_visual_result.stdout, "weak visual evidence should fail verification")

        currency_fixture = tmp / "currency-rates-fixture"
        make_currency_rates_fixture(currency_fixture)
        currency_out = tmp / "currency-rates-out"
        build(currency_fixture, currency_out)
        write_complete_reports(currency_out)
        write_currency_priority_visual_report(currency_out, include_p1=False)
        currency_priority_result = verify(currency_out, expect=1)
        check(
            "rendered journey usability danger terms require a visual/usability finding" in currency_priority_result.stdout,
            "currency rates rendered usability regression should require a visual/usability finding",
        )
        write_currency_priority_visual_report(currency_out, include_p1=True)
        currency_priority_fixed = verify(currency_out)
        check("ok: true" in currency_priority_fixed.stdout, "visual/usability finding should satisfy rendered usability regression")

        layout_noise_out = tmp / "layout-noise-out"
        build(ui_fixture, layout_noise_out)
        write_complete_reports(layout_noise_out)
        write_layout_noise_visual_report(layout_noise_out, include_p1=False)
        layout_noise_result = verify(layout_noise_out, expect=1)
        check(
            "rendered journey usability danger terms require a visual/usability finding" in layout_noise_result.stdout
            or "visual danger terms require a visual/usability finding" in layout_noise_result.stdout,
            "layout noise and disclosure instability should require a visual/usability finding",
        )
        write_layout_noise_visual_report(layout_noise_out, include_p1=True)
        layout_noise_fixed = verify(layout_noise_out)
        check("ok: true" in layout_noise_fixed.stdout, "layout-noise finding should satisfy visual verifier")

        out_of_scope_out = tmp / "out-of-scope-out"
        build(ui_fixture, out_of_scope_out, "--batch-size", "1")
        write_complete_reports(out_of_scope_out, out_of_scope=True)
        out_of_scope_result = verify(out_of_scope_out, expect=1)
        check("out_of_scope" in out_of_scope_result.stdout, "out-of-batch finding should fail verification")

        stale_out = tmp / "stale-out"
        build(ui_fixture, stale_out)
        write_complete_reports(stale_out)
        write(ui_fixture / "src" / "App.tsx", (ui_fixture / "src" / "App.tsx").read_text(encoding="utf-8") + "\n// changed\n")
        stale_result = verify(stale_out, expect=1)
        check("current_hash_mismatches" in stale_result.stdout, "stale input hashes should fail verification")

        formal_drift_fixture = tmp / "formal-drift-fixture"
        make_ui_fixture(formal_drift_fixture)
        formal_drift_out = tmp / "formal-drift-out"
        build(formal_drift_fixture, formal_drift_out)
        write_complete_reports(formal_drift_out)
        write(formal_drift_fixture / "formal-web-ui.json", json.dumps({"targets": [{"url": "http://changed.invalid"}]}, indent=2))
        formal_drift_result = verify(formal_drift_out, expect=2)
        check("formal_config hash does not match" in formal_drift_result.stderr, "formal config drift must invalidate the audit manifest")

        assert_ledger_mutation_fails(
            out,
            tmp,
            "weak-capability-ledger-out",
            lambda ledger: ledger["worker_capability_check"].update({"status": "pending"}),
        )
        assert_ledger_mutation_fails(
            out,
            tmp,
            "weak-visual-worker-ledger-out",
            lambda ledger: ledger["visual_comparison_worker"].update({"runtime_provenance": ""}),
        )
        assert_ledger_mutation_fails(
            out,
            tmp,
            "missing-lead-provenance-ledger-out",
            lambda ledger: ledger["lead"].update({"runtime_provenance": ""}),
        )
        assert_ledger_mutation_fails(
            out,
            tmp,
            "forbidden-worker-effort-ledger-out",
            lambda ledger: ledger["batch_workers"][0].update({"required_reasoning_effort": "low"}),
        )

        cli_fixture = tmp / "cli-fixture"
        make_cli_fixture(cli_fixture)
        cli_out = tmp / "cli-out"
        assert_inapplicable(cli_fixture, cli_out)
        backend_claim_out = tmp / "backend-claim-out"
        backend_claim = assert_inapplicable(
            cli_fixture,
            backend_claim_out,
            "--implemented-ui-file",
            "src/tool.py",
        )
        check(
            "recognized executable UI construct" in backend_claim.stderr,
            "an explicitly named backend source file must not qualify as implemented UI",
        )
        backend_markup_out = tmp / "backend-markup-claim-out"
        backend_markup = assert_inapplicable(
            cli_fixture,
            backend_markup_out,
            "--implemented-ui-file",
            "views/StatusScreen.py",
        )
        check(
            "recognized executable UI construct" in backend_markup.stderr,
            "UI-like names, state words, and markup inside a backend string must not qualify",
        )
        backend_override_out = tmp / "backend-override-claim-out"
        backend_override = assert_inapplicable(
            cli_fixture,
            backend_override_out,
            "--implemented-ui-override",
            "src/ledger.py",
            "find_ledger_violations",
            "audit_ledger",
        )
        check(
            "UI-specific framework type" in backend_override.stderr,
            "two unrelated backend identifiers must not satisfy the exceptional UI relationship",
        )

        evidence_fixture = tmp / "evidence-only-fixture"
        make_evidence_only_fixture(evidence_fixture)
        evidence_out = tmp / "evidence-only-out"
        evidence_result = assert_inapplicable(
            evidence_fixture,
            evidence_out,
            "--implemented-ui-file",
            "docs/mockups/contacts.html",
        )
        check(
            "not applicable" in evidence_result.stderr.lower() and "docs/mockups/contacts.html" in evidence_result.stderr,
            "mockup HTML must not qualify as implemented product UI",
        )

        style_out = tmp / "style-only-out"
        style_result = assert_inapplicable(
            evidence_fixture,
            style_out,
            "--implemented-ui-file",
            "src/theme.css",
        )
        check("styling" in style_result.stderr, "styles alone must not qualify as implemented product UI")

        story_fixture = tmp / "story-only-fixture"
        make_story_only_fixture(story_fixture)
        story_out = tmp / "story-only-out"
        story_result = assert_inapplicable(
            story_fixture,
            story_out,
            "--implemented-ui-file",
            "stories/ContactList.stories.tsx",
        )
        check(
            "not applicable" in story_result.stderr.lower() and "stories/ContactList.stories.tsx" in story_result.stderr,
            "Storybook/test evidence alone must not qualify",
        )

        starter_fixture = tmp / "starter-fixture"
        make_starter_fixture(starter_fixture)
        starter_out = tmp / "starter-out"
        starter_result = assert_inapplicable(
            starter_fixture,
            starter_out,
            "--implemented-ui-file",
            "src/App.tsx",
        )
        check("untouched framework starter" in starter_result.stderr, "untouched framework scaffold must not qualify")

        native_fixture = tmp / "native-ui-fixture"
        make_native_ui_fixture(native_fixture)
        native_out = tmp / "native-ui-out"
        native_manifest = build(native_fixture, native_out)
        check(
            native_manifest["ui_implementation_audit"]["implementation_gate"]["status"] == "passed",
            "substantive native UI should qualify",
        )

        precision_fixture = tmp / "gate-precision-fixture"
        make_gate_precision_fixture(precision_fixture)
        for name, rel_path in (
            ("short-component", "src/SignOut.tsx"),
            ("custom-components", "src/Dashboard.tsx"),
            ("custom-kebab-components", "src/VueDashboard.vue"),
            ("design-system-path", "src/design/components/ContactList.tsx"),
            ("customized-starter-brand", "src/BrandedPage.tsx"),
            ("customized-starter-with-data", "src/CustomizedStarter.tsx"),
            ("interactive-compose-email-screen", "src/ComposeEmail.tsx"),
            ("design-system-components", "src/DesignSystemContacts.tsx"),
            ("react-native-components", "src/NativeContacts.tsx"),
            ("uikit-representable", "Sources/ContactStack.swift"),
            ("java-swing", "src/ContactsPanel.java"),
            ("pyqt-widget", "src/ContactsWidget.py"),
            ("server-rendered-html", "templates/contacts.html"),
            ("todo-product-page", "src/TodoPage.tsx"),
            ("dynamic-contact-list", "src/DynamicContacts.tsx"),
            ("angular-inline-template", "src/ContactsComponent.ts"),
            ("lit-template", "src/ContactsElement.ts"),
            ("lit-functional-component", "src/LitFunctionalContacts.ts"),
            ("php-contact-list", "templates/contacts.php"),
            ("flask-inline-template", "src/contacts_flask.py"),
            ("flask-get-inline-template", "src/contacts_flask_get.py"),
            ("flask-blueprint-inline-template", "src/contacts_blueprint.py"),
            ("fastapi-html-response", "src/contacts_fastapi.py"),
            ("fastapi-keyword-html-response", "src/contacts_fastapi_keyword.py"),
            ("fastapi-status-keyword-html-response", "src/contacts_fastapi_status_keyword.py"),
            ("fastapi-fstring-html-response", "src/contacts_fastapi_fstring.py"),
            ("fastapi-nested-header-html-response", "src/contacts_fastapi_headers.py"),
            ("data-bound-php-paragraph", "templates/contact-paragraphs.php"),
            ("data-bound-php-span", "templates/contact-spans.php"),
            ("data-bound-php-div", "templates/contact-divs.php"),
            ("partial-swiftui-with-secondary-gap", "Sources/PartialContactsView.swift"),
            ("data-bound-partial-swiftui", "Sources/DataBoundPartialContactsView.swift"),
            ("custom-only-swiftui", "Sources/CustomOnlyContactsView.swift"),
            ("custom-grid-swiftui", "Sources/ContactsGridView.swift"),
            ("custom-row-swiftui", "Sources/ContactRowContainer.swift"),
            ("readonly-uikit", "Sources/ReadOnlyContactsController.swift"),
            ("uiview-subclass", "Sources/ContactsNativeView.swift"),
            ("typed-uikit-control", "Sources/TypedContactsController.swift"),
            ("typed-appkit-control", "Sources/TypedContactsAppKitController.swift"),
            ("dynamic-uikit-image", "Sources/ProfileImageController.swift"),
            ("named-uikit-image", "Sources/NamedProfileImageController.swift"),
            ("data-uikit-image", "Sources/DataProfileImageController.swift"),
            ("dynamic-compose", "src/DynamicCompose.kt"),
            ("dynamic-flutter", "src/DynamicFlutter.dart"),
            ("dynamic-streamlit", "src/DynamicStreamlit.py"),
            ("dynamic-dom", "src/DynamicDomNode.js"),
            ("bound-native-list", "res/layout/bound-list.xml"),
        ):
            precision_out = tmp / f"{name}-out"
            precision_manifest = build(precision_fixture, precision_out, evidence=rel_path)
            check(
                precision_manifest["ui_implementation_audit"]["implementation_gate"]["status"] == "passed",
                f"legitimate {name} implementation should qualify",
            )

        unrecognized_out = tmp / "unrecognized-ui-out"
        unrecognized_result = assert_inapplicable(
            precision_fixture,
            unrecognized_out,
            "--implemented-ui-file",
            "src/contact_surface.canvas",
        )
        check(
            "explicit source-anchor override" in unrecognized_result.stderr,
            "an unrecognized framework must require the exceptional source-anchor path",
        )
        override_out = tmp / "explicit-source-anchor-out"
        override_manifest = build(
            precision_fixture,
            override_out,
            override=("src/contact_surface.canvas", "canvas_kit::ContactSurface", "build_contacts"),
        )
        override_evidence = override_manifest["ui_implementation_audit"]["implementation_gate"]["evidence_files"][0]
        check(
            override_evidence["qualification"]
            == {
                "method": "explicit-source-anchor",
                "detector_version": 2,
                "ui_kind": "canvas_kit::ContactSurface",
                "source_anchor": "build_contacts",
            },
            "exceptional qualification must persist its exact source basis",
        )
        forged_override_out = tmp / "forged-explicit-source-anchor-out"
        shutil.copytree(override_out, forged_override_out)
        forged_override_manifest_path = forged_override_out / "manifest.json"
        forged_override_manifest = json.loads(forged_override_manifest_path.read_text(encoding="utf-8"))
        forged_override_manifest["ui_implementation_audit"]["implementation_gate"]["evidence_files"][0][
            "qualification"
        ]["source_anchor"] = "missing_contact_anchor"
        write(forged_override_manifest_path, json.dumps(forged_override_manifest, indent=2))
        forged_override_result = verify(forged_override_out, expect=2)
        check(
            "not a qualifying product UI source" in forged_override_result.stderr,
            "verifier must revalidate an exceptional source anchor against current source",
        )

        for name, rel_path in (
            ("route-scaffold", "src/RouteOnly.tsx"),
            ("null-route", "src/NullRoute.tsx"),
            ("root-mount", "src/RootMount.tsx"),
            ("strict-root-mount", "src/StrictRoot.tsx"),
            ("provider-route-scaffold", "src/AuthRoute.tsx"),
            ("wrapped-root-mount", "src/WrappedRoot.tsx"),
            ("wrapped-route-scaffold", "src/WrappedRoute.tsx"),
            ("custom-root-screen", "src/RootScreen.tsx"),
            ("custom-layout-page", "src/LayoutPage.tsx"),
            ("custom-auth-route", "src/CustomAuthRoute.tsx"),
            ("multi-tag-placeholder", "src/ComingSoon.tsx"),
            ("titled-placeholder", "src/TitledPlaceholder.tsx"),
            ("verbose-placeholder", "src/VerbosePlaceholder.tsx"),
            ("accessible-placeholder", "src/AccessiblePlaceholder.tsx"),
            ("under-construction-placeholder", "src/UnderConstruction.tsx"),
            ("work-in-progress-placeholder", "src/WorkInProgress.tsx"),
            ("no-op-handler-placeholder", "src/NoOpPlaceholder.tsx"),
            ("pyqt-import-only", "src/PyQtImports.py"),
            ("empty-swiftui-shell", "Sources/EmptyContactsView.swift"),
            ("placeholder-swiftui-shell", "Sources/PlaceholderContactsView.swift"),
            ("empty-swiftui-container", "Sources/EmptyStackView.swift"),
            ("empty-native-representable", "Sources/EmptyRepresentable.swift"),
            ("empty-swing-shell", "src/EmptyContactsPanel.java"),
            ("swing-layout-only", "src/LayoutOnlyPanel.java"),
            ("swing-container-only", "src/ContainerOnlyPanel.java"),
            ("qt-layout-only", "src/LayoutOnlyWidget.py"),
            ("empty-uiview-controller", "Sources/EmptyController.swift"),
            ("empty-swiftui-text", "Sources/EmptyTextView.swift"),
            ("empty-swiftui-verbatim-text", "Sources/EmptyVerbatimTextView.swift"),
            ("whitespace-swiftui-verbatim-text", "Sources/WhitespaceVerbatimTextView.swift"),
            ("escaped-whitespace-swiftui-verbatim-text", "Sources/EscapedWhitespaceVerbatimTextView.swift"),
            ("unattached-uikit-control", "Sources/UnusedButtonController.swift"),
            ("blank-attached-uikit-control", "Sources/BlankAttachedButtonController.swift"),
            ("blank-constructed-uikit-image", "Sources/EmptyProfileImageController.swift"),
            ("blank-data-uikit-image", "Sources/EmptyDataProfileImageController.swift"),
            ("blank-assigned-uikit-image", "Sources/AssignedEmptyProfileImageController.swift"),
            ("blank-uikit-with-debug-text", "Sources/BlankButtonWithDebugTextController.swift"),
            ("unattached-swing-control", "src/UnusedSwingButton.java"),
            ("blank-attached-swing-control", "src/BlankAttachedSwingButton.java"),
            ("blank-swing-with-debug-text", "src/BlankSwingButtonWithDebugText.java"),
            ("unattached-qt-control", "src/UnusedQtLabel.py"),
            ("blank-attached-qt-control", "src/BlankAttachedQtLabel.py"),
            ("blank-qt-with-debug-text", "src/BlankQtLabelWithDebugText.py"),
            ("blank-compose-control", "src/EmptyCompose.kt"),
            ("blank-compose-alias", "src/EmptyAliasCompose.kt"),
            ("blank-compose-constructor-alias", "src/EmptyConstructorAliasCompose.kt"),
            ("blank-compose-alias-chain", "src/EmptyAliasChainCompose.kt"),
            ("blank-compose-with-debug-text", "src/EmptyComposeWithDebugText.kt"),
            ("blank-flutter-control", "src/EmptyFlutter.dart"),
            ("blank-flutter-alias", "src/EmptyAliasFlutter.dart"),
            ("blank-streamlit-control", "src/EmptyStreamlit.py"),
            ("blank-streamlit-alias", "src/EmptyAliasStreamlit.py"),
            ("blank-streamlit-with-debug-text", "src/EmptyStreamlitWithDebugText.py"),
            ("blank-gradio-control", "src/EmptyGradioMarkdown.py"),
            ("blank-tk-control", "src/EmptyTkButton.py"),
            ("blank-dom-control", "src/EmptyDomNode.js"),
            ("blank-dom-alias", "src/EmptyAliasDomNode.js"),
            ("blank-dom-with-debug-text", "src/EmptyDomNodeWithDebugText.js"),
            ("empty-android-control", "res/layout/empty-control.xml"),
            ("empty-native-list", "res/layout/empty-list.xml"),
            ("whitespace-bound-native-list", "res/layout/whitespace-list.xml"),
            ("backend-email-template", "src/WelcomeEmail.ts"),
            ("tagged-backend-email-template", "src/TaggedWelcomeEmail.ts"),
            ("lit-tagged-backend-email-template", "src/LitTaggedWelcomeEmail.ts"),
            ("pascal-lit-backend-email-template", "src/PascalLitWelcomeEmail.ts"),
            ("pascal-jsx-backend-email-template", "src/PascalJsxWelcomeEmail.tsx"),
            ("email-view-backend-template", "src/email.ts"),
            ("empty-web-control", "templates/empty-button.html"),
            ("streamlit-config-only", "src/StreamlitConfig.py"),
            ("empty-gradio-blocks", "src/EmptyGradio.py"),
            ("streamlit-placeholder", "src/StreamlitPlaceholder.py"),
            ("gradio-placeholder", "src/GradioPlaceholder.py"),
            ("dom-placeholder", "src/DomPlaceholder.js"),
            ("xml-placeholder", "res/layout/placeholder.xml"),
            ("empty-attributed-button", "templates/empty-attributed-button.html"),
            ("empty-link", "templates/empty-link.html"),
            ("empty-tk-frame", "src/EmptyTk.py"),
        ):
            scaffold_out = tmp / f"{name}-out"
            scaffold_result = assert_inapplicable(
                precision_fixture,
                scaffold_out,
                "--implemented-ui-file",
                rel_path,
            )
            check(
                "not applicable" in scaffold_result.stderr.lower(),
                f"{name} must not qualify as a substantive product UI surface",
            )

        view_model_override_out = tmp / "view-model-override-out"
        view_model_override = assert_inapplicable(
            cli_fixture,
            view_model_override_out,
            "--implemented-ui-override",
            "src/ledger_view_model.py",
            "LedgerViewModel",
            "audit_ledger",
        )
        check(
            "UI-specific framework type" in view_model_override.stderr,
            "backend ViewModel types must not satisfy the exceptional UI kind",
        )

        unrelated_override_out = tmp / "unrelated-override-out"
        unrelated_override = assert_inapplicable(
            precision_fixture,
            unrelated_override_out,
            "--implemented-ui-override",
            "src/unrelated_surface.canvas",
            "canvas_kit::ContactSurface",
            "audit_ledger",
        )
        check(
            "definition does not use" in unrelated_override.stderr,
            "an unrelated later UI use must not qualify the named backend definition",
        )

        local_view_override_out = tmp / "local-view-override-out"
        local_view_override = assert_inapplicable(
            precision_fixture,
            local_view_override_out,
            "--implemented-ui-override",
            "src/local_report_view.py",
            "ReportView",
            "render_report",
        )
        check(
            "local name alone is not framework evidence" in local_view_override.stderr,
            "a locally declared backend projection with a UI-shaped suffix must not satisfy the exceptional gate",
        )

        database_view_override_out = tmp / "database-view-override-out"
        database_view_override = assert_inapplicable(
            precision_fixture,
            database_view_override_out,
            "--implemented-ui-override",
            "src/database_view.canvas",
            "database::MaterializedView",
            "build_contacts",
        )
        check(
            "UI-specific framework type" in database_view_override.stderr,
            "a namespace-qualified database view must not satisfy the exceptional UI gate",
        )

        sqlx_view_override_out = tmp / "sqlx-view-override-out"
        sqlx_view_override = assert_inapplicable(
            precision_fixture,
            sqlx_view_override_out,
            "--implemented-ui-override",
            "src/sqlx_view.canvas",
            "sqlx::ContactView",
            "build_contacts",
        )
        check(
            "UI-specific framework type" in sqlx_view_override.stderr,
            "a SQL client projection must not satisfy the exceptional UI gate",
        )

        sea_orm_view_override_out = tmp / "sea-orm-view-override-out"
        sea_orm_view_override = assert_inapplicable(
            precision_fixture,
            sea_orm_view_override_out,
            "--implemented-ui-override",
            "src/sea_orm_view.canvas",
            "sea_orm::ContactView",
            "build_contacts",
        )
        check(
            "UI-specific framework type" in sea_orm_view_override.stderr,
            "an ORM projection must not satisfy the exceptional UI gate",
        )

        imported_database_view_override_out = tmp / "imported-database-view-override-out"
        imported_database_view_override = assert_inapplicable(
            precision_fixture,
            imported_database_view_override_out,
            "--implemented-ui-override",
            "src/imported_database_view.py",
            "ReportView",
            "build_contacts",
        )
        check(
            "imported from a data" in imported_database_view_override.stderr,
            "a backend projection imported from a database module must not satisfy the exceptional UI gate",
        )

        imgui_out = tmp / "imgui-override-out"
        imgui_manifest = build(
            precision_fixture,
            imgui_out,
            override=("src/contacts_imgui.cpp", "ImGui::Button", "ContactsScreen"),
        )
        check(
            imgui_manifest["ui_implementation_audit"]["implementation_gate"]["status"] == "passed",
            "a structurally bound uncommon immediate-mode UI should qualify",
        )

        partial_override_out = tmp / "partial-explicit-source-anchor-out"
        partial_override_manifest = build(
            precision_fixture,
            partial_override_out,
            override=("src/partial_contacts.canvas", "canvas_kit::ContactSurface", "render_contacts"),
        )
        check(
            partial_override_manifest["ui_implementation_audit"]["implementation_gate"]["status"] == "passed",
            "a data-bound uncommon UI with a secondary incomplete feature should still qualify",
        )

        dynamic_partial_override_out = tmp / "dynamic-partial-explicit-source-anchor-out"
        dynamic_partial_override_manifest = build(
            precision_fixture,
            dynamic_partial_override_out,
            override=(
                "src/dynamic_partial_contacts.canvas",
                "canvas_kit::ContactSurface",
                "build_contacts",
            ),
        )
        check(
            dynamic_partial_override_manifest["ui_implementation_audit"]["implementation_gate"]["status"]
            == "passed",
            "a manually attested dynamic partial UI must not depend on a hard-coded iteration vocabulary",
        )

        placeholder_out = tmp / "placeholder-only-out"
        placeholder_result = assert_inapplicable(
            precision_fixture,
            placeholder_out,
            "--implemented-ui-file",
            "src/Preview.tsx",
        )
        check("placeholder" in placeholder_result.stderr, "comment-padded placeholder UI must not qualify")

        print("self-test ok")
        return 0
    finally:
        if KEEP_TEMP_ON_FAILURE:
            print(f"Preserved self-test workspace: {tmp}", file=sys.stderr)
        else:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
