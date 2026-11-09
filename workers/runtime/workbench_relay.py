"""Fixed-destination TCP relay; the tool container never joins the egress network."""
import asyncio
import sys

async def main(host,port):
    slots=asyncio.Semaphore(32)
    async def connection(reader,writer):
        async with slots:
            upstream=None
            try:
                remote,upstream=await asyncio.wait_for(asyncio.open_connection(host,port),10)
                async def copy(source,destination):
                    while data:=await asyncio.wait_for(source.read(65536),300):
                        destination.write(data); await destination.drain()
                    if destination.can_write_eof(): destination.write_eof()
                await asyncio.gather(copy(reader,upstream),copy(remote,writer))
            except (OSError,asyncio.TimeoutError): pass
            finally:
                writer.close()
                if upstream: upstream.close()
    server=await asyncio.start_server(connection,"0.0.0.0",port,limit=65536)
    async with server: await server.serve_forever()

if __name__=="__main__": asyncio.run(main(sys.argv[1],int(sys.argv[2])))
