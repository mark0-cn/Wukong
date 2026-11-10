// A process-level fixture for installer transactions, never used in a release.
using System;
using System.IO;
class InstallerClient {
    static void Main(string[] args) {
        string config = args[1];
        string root = Path.GetDirectoryName(config);
        string state = Path.Combine(root, "fixture-image");
        string command = args[3];
        if (command == "status") {
            Console.WriteLine(File.Exists(state) ? "{\"environment\":{\"image_id\":\"" + File.ReadAllText(state) + "\"}}" : "{\"environment\":null}");
        } else if (command == "up" || command == "update") {
            string image = args[5];
            image = "sha256:" + image.Substring(image.LastIndexOf(':') + 1);
            File.WriteAllText(state, image);
            string generated = Path.Combine(root, "generated.toml");
            File.Copy(config, generated, true);
            Console.WriteLine("{\"config\":\"" + generated.Replace("\\", "\\\\") + "\"}");
        } else if (command == "doctor") {
            bool fail = File.Exists(Path.Combine(root, "fail-doctor")) && File.ReadAllText(state).EndsWith(new string('b', 64));
            Console.WriteLine(fail ? "{\"passed\":false}" : "{\"passed\":true}");
        } else if (command == "down") {
            Console.WriteLine("{}");
        } else {
            Environment.Exit(2);
        }
    }
}
