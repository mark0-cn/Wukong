// Wukong headless JSON bridge. Invoked by Rust with request and response paths.
// @category Wukong
import ghidra.app.util.headless.HeadlessScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import com.google.gson.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

public class WukongBridge extends HeadlessScript {
    private String targetHash;
    private JsonObject address(Address a) {
        JsonObject out = new JsonObject();
        out.addProperty("link_time_address", a.toString());
        out.addProperty("ghidra_address_space", a.getAddressSpace().getName());
        if (!a.getAddressSpace().equals(currentProgram.getImageBase().getAddressSpace())) {
            // Imported/external, register and stack spaces have no meaningful
            // module-relative offset. Keep their symbolic identity explicitly.
            out.addProperty("space", "ghidra_other");
            out.addProperty("symbolic_address", a.toString());
            return out;
        }
        out.addProperty("module_sha256", targetHash);
        out.addProperty("space", "module_relative");
        out.addProperty("value", a.subtract(currentProgram.getImageBase()));
        out.add("runtime_instance_id", JsonNull.INSTANCE);
        out.add("load_base", JsonNull.INSTANCE);
        return out;
    }
    private Address resolve(JsonObject query) throws Exception {
        long offset = Long.parseUnsignedLong(query.get("address").getAsString());
        Address a = currentProgram.getImageBase().addNoWrap(offset);
        if (!currentProgram.getMemory().contains(a)) throw new IllegalArgumentException("module-relative address outside program memory");
        return a;
    }
    private JsonObject function(Function f) {
        JsonObject out = new JsonObject();
        out.addProperty("name", f.getName(true));
        out.add("address", address(f.getEntryPoint()));
        out.addProperty("signature", f.getSignature().toString());
        out.addProperty("external", f.isExternal());
        out.addProperty("body_bytes", f.getBody().getNumAddresses());
        return out;
    }
    private JsonObject reference(Reference ref) {
        JsonObject out = new JsonObject();
        out.add("from", address(ref.getFromAddress()));
        out.add("to", address(ref.getToAddress()));
        out.addProperty("type", ref.getReferenceType().toString());
        out.addProperty("operand", ref.getOperandIndex());
        return out;
    }
    @Override public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 2) throw new IllegalArgumentException("expected request.json response.json");
        JsonObject out = new JsonObject();
        try {
            JsonObject request = JsonParser.parseString(Files.readString(Path.of(args[0]), StandardCharsets.UTF_8)).getAsJsonObject();
            targetHash = request.get("target_sha256").getAsString();
            if (currentProgram == null) throw new IllegalStateException("no imported program");
            // A timed-out import is never published as a completed analysis cache.
            if (analysisTimeoutOccurred()) throw new IllegalStateException("Ghidra automatic analysis timed out; increase analysis_timeout_seconds");
            String actualHash = currentProgram.getExecutableSHA256();
            if (actualHash == null || !targetHash.equalsIgnoreCase(actualHash)) throw new IllegalStateException("imported program SHA256 does not match request");
            out.addProperty("target_sha256", targetHash);
            out.addProperty("image_base", currentProgram.getImageBase().toString());
            out.addProperty("language", currentProgram.getLanguageID().toString());
            out.addProperty("compiler", currentProgram.getCompilerSpec().getCompilerSpecID().toString());
            JsonObject q = request.getAsJsonObject("query");
            String op = q.get("operation").getAsString();
            out.addProperty("operation", op);
            if ("functions".equals(op)) {
                int limit = q.get("limit").getAsInt();
                long offset = q.get("offset").getAsLong();
                if (limit < 1 || limit > 5000 || offset < 0) throw new IllegalArgumentException("invalid function pagination");
                String pattern = q.has("pattern") && !q.get("pattern").isJsonNull() ? q.get("pattern").getAsString() : "";
                JsonArray items = new JsonArray();
                FunctionIterator iter = currentProgram.getFunctionManager().getFunctions(true);
                long matches = 0;
                boolean more = false;
                while (iter.hasNext()) {
                    monitor.checkCancelled();
                    Function f = iter.next();
                    if (!f.getName(true).contains(pattern)) continue;
                    if (matches++ < offset) continue;
                    if (items.size() == limit) { more = true; break; }
                    items.add(function(f));
                }
                out.add("functions", items);
                out.addProperty("returned", items.size());
                out.addProperty("total_functions", currentProgram.getFunctionManager().getFunctionCount());
                out.addProperty("truncated", more);
                if (more) out.addProperty("next_offset", offset + items.size());
            } else if ("decompile".equals(op)) {
                Address a = resolve(q);
                Function f = currentProgram.getFunctionManager().getFunctionContaining(a);
                if (f == null) throw new IllegalArgumentException("address is not within a discovered function");
                DecompInterface decompiler = new DecompInterface();
                try {
                    if (!decompiler.openProgram(currentProgram)) throw new IllegalStateException("decompiler could not open program");
                    DecompileResults r = decompiler.decompileFunction(f, 60, monitor);
                    if (!r.decompileCompleted() || r.getDecompiledFunction() == null) throw new IllegalStateException("decompile failed: " + r.getErrorMessage());
                    String code = r.getDecompiledFunction().getC();
                    // Full code is a sibling artifact; JSON returned to the model is bounded.
                    Path artifact = Path.of(args[1]).resolveSibling("decompile.c");
                    Files.writeString(artifact, code, StandardCharsets.UTF_8);
                    out.add("function", function(f));
                    out.addProperty("code", code.substring(0, Math.min(code.length(), 65536)));
                    out.addProperty("code_chars", code.length());
                    out.addProperty("truncated", code.length() > 65536);
                    out.addProperty("artifact_path", artifact.toAbsolutePath().toString());
                } finally { decompiler.dispose(); }
            } else if ("instructions".equals(op)) {
                Function f = currentProgram.getFunctionManager().getFunctionContaining(resolve(q));
                if (f == null) throw new IllegalArgumentException("no containing function");
                int limit = q.get("limit").getAsInt(); long offset = q.get("offset").getAsLong();
                if (limit < 1 || limit > 5000 || offset < 0) throw new IllegalArgumentException("invalid pagination");
                JsonArray items = new JsonArray(); long seen = 0; boolean more = false;
                var blockModel = new ghidra.program.model.block.BasicBlockModel(currentProgram);
                var iter = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (iter.hasNext()) {
                    monitor.checkCancelled(); var instruction = iter.next();
                    if (seen++ < offset) continue;
                    if (items.size() == limit) { more = true; break; }
                    JsonObject row = new JsonObject();
                    row.add("address", address(instruction.getAddress()));
                    row.addProperty("text", instruction.toString());
                    var block = blockModel.getFirstCodeBlockContaining(instruction.getAddress(), monitor);
                    if (block != null) row.add("block_start", address(block.getFirstStartAddress()));
                    row.addProperty("flow_type", instruction.getFlowType().toString());
                    JsonArray flows = new JsonArray();
                    for (Address flow : instruction.getFlows()) flows.add(address(flow));
                    row.add("flows", flows);
                    if (instruction.getFallThrough() != null) row.add("fallthrough", address(instruction.getFallThrough()));
                    items.add(row);
                }
                out.add("function", function(f)); out.add("instructions", items);
                out.addProperty("truncated", more);
                if (more) out.addProperty("next_offset", offset + items.size());
            } else if ("xrefs".equals(op)) {
                Address a = resolve(q);
                int limit = q.get("limit").getAsInt();
                if (limit < 1 || limit > 5000) throw new IllegalArgumentException("invalid xref limit");
                String direction = q.get("direction").getAsString();
                if (!direction.equals("to") && !direction.equals("from") && !direction.equals("both")) throw new IllegalArgumentException("invalid xref direction");
                JsonArray refs = new JsonArray(); boolean more = false;
                if (!direction.equals("from")) {
                    ReferenceIterator iter = currentProgram.getReferenceManager().getReferencesTo(a);
                    while (iter.hasNext()) {
                        monitor.checkCancelled();
                        Reference r = iter.next();
                        if (refs.size() == limit) { more = true; break; }
                        refs.add(reference(r));
                    }
                }
                if (!direction.equals("to")) {
                    Reference[] outgoing = currentProgram.getReferenceManager().getReferencesFrom(a);
                    for (Reference r : outgoing) {
                        monitor.checkCancelled();
                        if (refs.size() == limit) { more = true; break; }
                        refs.add(reference(r));
                    }
                }
                out.add("address", address(a)); out.add("references", refs);
                out.addProperty("returned", refs.size()); out.addProperty("truncated", more);
            } else throw new IllegalArgumentException("unsupported operation: " + op);
            out.addProperty("ok", true);
        } catch (Exception e) {
            out.addProperty("ok", false);
            out.addProperty("error", e.getClass().getName() + ": " + e.getMessage());
            printerr(e.toString());
        }
        Files.writeString(Path.of(args[1]), new GsonBuilder().setPrettyPrinting().create().toJson(out), StandardCharsets.UTF_8);
    }
}
