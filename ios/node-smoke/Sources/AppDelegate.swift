import UIKit
import Darwin
import NodeMobile

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        let w = UIWindow(frame: UIScreen.main.bounds)
        w.rootViewController = UIViewController()
        w.makeKeyAndVisible()
        window = w

        Thread.detachNewThread {
            let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            docs.path.withCString { setenv("HOME", $0, 1) }
            guard let script = Bundle.main.path(forResource: "smoke", ofType: "js") else {
                try? "missing smoke.js".write(to: docs.appendingPathComponent("smoke-error.txt"),
                                               atomically: true, encoding: .utf8)
                return
            }
            let args = ["node", script]
            var argv: [UnsafeMutablePointer<CChar>?] = args.map { strdup($0) }
            argv.append(nil)
            let rc = node_start(Int32(args.count), &argv)
            for p in argv { if let p { free(p) } }
            let result = "rc=\(rc) node=\(ProcessInfo.processInfo.environment["NODE_VERSION"] ?? "?")"
            try? result.write(to: docs.appendingPathComponent("smoke-result.txt"),
                              atomically: true, encoding: .utf8)
        }
        return true
    }
}