本目录证书与私钥仅用于回环 TLS 测试，域名 fixture.example。私钥是公开测试夹具，绝不能用于部署。root.der 为测试 CA，cert.der/key.der 为测试服务端证书/私钥。测试显式信任该 CA；生产适配器不添加此根证书，也不禁用证书/主机名验证。
