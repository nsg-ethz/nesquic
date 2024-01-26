echo "Compile sendmsg"
gcc sendmsg.c

echo "Enable TSO"
sudo ethtool -K enp1s0 tx on sg on gso on

echo "Increase tx buffer"
sudo sysctl -w net.core.wmem_max=2097152

./a.out
# strace --trace=write,sendmsg,sendmmsg ./a.out

rm a.out