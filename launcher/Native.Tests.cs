// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

using System;
using System.Linq;
namespace Afterhours.Windows.Tests
{
    public static class Transactions
    {
        private static void Check(bool value, string description) { if (!value) throw new Exception(description); }
        private static void Refuses(Action action) {
            bool refused = false; try { action(); } catch (InvalidOperationException) { refused = true; }
            Check(refused, "Expected refusal");
        }
        private static byte[][] Original() { return new[] {new byte[] {0,9,8,7,6,5,4,3,2,1}, new byte[] {1}, new byte[] {0,0}}; }
        private static bool Equal(byte[][] a, byte[][] b) { return Enumerable.Range(0,3).All(i => a[i].SequenceEqual(b[i])); }
        public static void Run()
        {
            // Each failing write can modify memory before reporting failure.
            for (int failedWrite = 1; failedWrite <= 3; failedWrite++) {
                var memory = Original(); int writes = 0; bool fault = true;
                var lease = new EndpointTransaction(i => (byte[])memory[i].Clone(), (i, value) => {
                    writes++;
                    if (fault && writes == failedWrite) {
                        Array.Copy(value, memory[i], Math.Max(1, value.Length / 2)); fault = false;
                        throw new InvalidOperationException("Injected partial write");
                    }
                    memory[i] = (byte[])value.Clone();
                }, () => {}, 0x1234);
                Refuses(lease.Apply);
                lease.Restore(); Check(Equal(memory, Original()), "Partial write rollback preserves original padding");
                int restoredWrites = writes; lease.Restore(); Check(writes == restoredWrites, "Rollback is idempotent");
            }
            {
                var memory = Original(); int writes = 0;
                var lease = new EndpointTransaction(i => (byte[])memory[i].Clone(), (i,v) => { writes++; memory[i] = v; }, () => {}, 32000);
                lease.Apply(); Check(memory[0][0] == (byte)'1', "Endpoint host starts with ASCII 1");
                memory[0][0] = 250; int before = writes;
                Refuses(lease.Restore); Check(writes == before, "Never overwrite another writer's data");
            }
            {
                var memory = Original(); int writes = 0; bool alive = true;
                var lease = new EndpointTransaction(i => (byte[])memory[i].Clone(), (i,v) => { writes++; memory[i] = v; },
                    () => { if (!alive) throw new InvalidOperationException("Process/root identity changed"); }, 32000);
                alive = false; Refuses(lease.Apply); Check(writes == 0, "Identity failure must precede writes");
            }
            {
                var memory = Original(); int writes = 0;
                var lease = new EndpointTransaction(i => (byte[])memory[i].Clone(), (i,v) => { writes++; memory[i] = v; }, () => {}, 32000);
                lease.Apply(); Check(!lease.IsOriginal(), "Applied endpoint differs"); Refuses(lease.Apply);
                lease.Restore(); Check(Equal(memory, Original()), "Complete rollback");
                Refuses(() => new EndpointTransaction(i => memory[i], (i,v) => {}, () => {}, 80));
                memory[1][0] = 2; Refuses(() => new EndpointTransaction(i => memory[i], (i,v) => {}, () => {}, 32000));
            }
        }
    }
}
