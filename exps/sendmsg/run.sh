echo "Compile sendmsg"
gcc tcp_server.c -o tcp_server
gcc sendmsg.c -o sendmsg

echo "Enable TSO"
sudo ethtool -K enp1s0 tx on sg on gso on

echo "Increase tx buffer"
sudo sysctl -w net.core.wmem_max=2097152

./tcp_server &
./sendmsg
# strace --trace=write,sendmsg,sendmmsg ./a.out

kill %1
rm tcp_server
rm sendmsg