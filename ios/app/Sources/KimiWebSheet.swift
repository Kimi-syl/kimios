// Full-screen WKWebView for the local OpenCode web UI. The local server uses
// HTTP Basic auth; WKWebView does not prompt automatically, so answer the
// authentication challenge with the Node server password.

import UIKit
import WebKit

final class KimiWebSheet: UIViewController, WKNavigationDelegate, WKUIDelegate {
    private let target: URL
    private let username: String
    private let password: String
    private var webView: WKWebView!

    init(url: URL, username: String, password: String) {
        self.target = url
        self.username = username
        self.password = password
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let config = WKWebViewConfiguration()
        config.allowsInlineMediaPlayback = true
        webView = WKWebView(frame: view.bounds, configuration: config)
        webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        webView.navigationDelegate = self
        webView.uiDelegate = self
        view.addSubview(webView)
        webView.load(URLRequest(url: target))
    }

    func webView(_ webView: WKWebView, didReceive challenge: URLAuthenticationChallenge,
                 completionHandler: @escaping (URLSession.AuthChallengeDisposition,
                                               URLCredential?) -> Void) {
        let method = challenge.protectionSpace.authenticationMethod
        if method == NSURLAuthenticationMethodHTTPBasic ||
            method == NSURLAuthenticationMethodHTTPDigest {
            let credential = URLCredential(user: username, password: password,
                                           persistence: .forSession)
            completionHandler(.useCredential, credential)
            return
        }
        completionHandler(.performDefaultHandling, nil)
    }

    static func present(url: URL, password: String) {
        DispatchQueue.main.async {
            guard let top = topViewController() else { return }
            let sheet = KimiWebSheet(url: url, username: "opencode", password: password)
            sheet.modalPresentationStyle = .fullScreen
            top.present(sheet, animated: true)
        }
    }

    private static func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let window = scenes.flatMap { $0.windows }.first(where: { $0.isKeyWindow })
            ?? scenes.first?.windows.first
        var top = window?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        return top
    }
}